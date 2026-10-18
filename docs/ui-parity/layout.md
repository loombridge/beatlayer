# Layout

Where every view, panel, and control sits in Live 12, so Beatlayer can put it in the same place (D16). Read the [README](README.md) first: it explains the IDs, the sources, and how measurements were taken.

## How to read this page

- **Measurements** are in points, from the top-left corner of the main window, with the window set to **1600 × 1000 points** (its outer frame, title bar included) on macOS, in Live 12.4.3, on 10 October 2026. They were read from Live's accessibility tree (element frames) and from screen captures used only for measuring; no capture is committed. Values are rounded to the nearest point and carry about ±2 points of error.
- **Proportions** matter more than absolute values: panes that resize with the window are given as a share of the window too. Beatlayer matches the proportions at any size and the absolute values at 1600 × 1000.
- **Zoom.** Measurements are at Live's 100% interface zoom (Display & Input's Zoom, L-SET-010). At any other zoom every row scales by it, keeping its place relative to the others ([interface-zoom](../specs/interface-zoom.md)).
- **Default state** means a new, empty set as Live opens it: four tracks (two MIDI, two audio), two return tracks, eight scenes, the browser open on its Sounds label, the Session View in front, an empty Device View at the bottom, the Info View open, and, on a first launch, the Learn View on the right. Measurements are given with the Learn View closed unless a row says otherwise, because that's the layout people work in.
- **Source**: `obs` means observed in Live 12.4.3 as described above; `man N` means section N of the Live 12 reference manual, paraphrased.
- **Shown or hidden by** names the menu item (M- IDs in [menus.md](menus.md)), the shortcut (S- IDs in [shortcuts.toml](shortcuts.toml)), or the on-screen toggle that controls the element.
- Controls are listed **in on-screen order**: left to right, then top to bottom.

## Main window (L-WIN)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-WIN-001 | Main window | One window holds everything below. Opens to the last-used size; the title is the set's name ("Untitled" for a new set). | Always present | obs |
| L-WIN-002 | Title bar | Top 23 pt. Standard window buttons at the left (close, minimise, full screen), set name centred. No toolbar. Beatlayer draws it on macOS (D22): the content covers the window, the system's buttons sit centred in the 23 pt strip at x 9, 32, and 55 (14 pt each, AppKit's spacing), and dragging and double-clicking stay the system's. On Windows and Linux Beatlayer keeps the system's title bar. | Always present; hidden in full screen (S-VIEW-001) | obs |
| L-WIN-003 | Control Bar | One row under the title bar, full window width; its controls are 18 pt tall and centred about 37 pt from the top. Views start at 61 pt. | Always present | obs, man 3.1 |
| L-WIN-004 | Browser column | Left edge, 9 to 311 pt (302 pt, 19% of the width), from 61 pt down to the top of the detail area (766 pt by default). Can be widened by dragging its right edge, or run to the bottom of the window (Full-Height Browser, M-VIEW-024). | Browser toggle at the left of the Control Bar (L-CTRL-001), M-VIEW-021, S-VIEW-010 | obs, man 4 |
| L-WIN-005 | Main view (Session or Arrangement) | Right of the browser, 320 to 1591 pt (1271 pt, 79% of the width) with the Learn View closed, or 320 to 1287 pt with it open; from 61 pt down to the detail area (766 pt by default). A 9 pt gap separates it from the browser. | Always present; which view shows is set by the view selector (L-CTRL-027), S-VIEW-003 | obs |
| L-WIN-006 | Detail area | Under the main view and the browser: Info View at the left, then the Clip View or Device View. Default 774 to 962 pt (188 pt, 19% of the height) with an empty Device View; 738 to 963 pt (225 pt) once a clip shows in the Clip View. Resized by dragging its top edge; dragging it to the bottom closes it. | Clip and Device View toggles in the Status Bar (L-STAT-004 to 007), S-VIEW-015, S-VIEW-016; hidden whole by M-VIEW-014, S-VIEW-022 | obs, man 8, 23.1 |
| L-WIN-007 | Status Bar | Bottom row, 968 to 993 pt (25 pt), full width with the Learn View closed; it stops at the Learn View's left edge when that's open. | Always present | obs, man 3.2 |
| L-WIN-008 | Learn View column | Right edge, 1296 to 1591 pt (295 pt, 18% of the width), from 61 pt to the window's bottom edge, beside both the main view and the detail area. The Undo History opens in the same right-hand area (L-OTH-001). | Its close button, M-VIEW-005, M-HELP-001, S-VIEW-018; shown on a first launch | obs, man 2.2.1 |
| L-WIN-009 | Margins | 9 pt between panes and at the window's left and right edges; 8 pt between the main view and the detail area. | | obs |
| D24 | Assistant pane | Beatlayer addition. Right edge, from 61 pt to the window's bottom edge, beside the main view, the detail area and the Status Bar, which stop 9 pt left of it (as beside the Learn View, L-WIN-008). Width 240 to 640 pt, 320 by default, set by dragging its left edge (or Left and Right on the focused edge) and kept as an application setting. Its header's Float and Dock button moves it into a window of its own and back; the choice is kept. A new session starts with it closed | The Control Bar's assistant toggle (D24 row below), View ▸ Assistant, `view.toggle_assistant` | Beatlayer addition, decided 16 October 2026 (D24) |

## Control Bar (L-CTRL)

Nine groups, left to right (man 3.1). x positions are the left edges of each control at 1600 × 1000 with the Learn View closed; the Control Bar doesn't change with the Learn View. Gaps between groups stretch with the window; controls keep their size.

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-CTRL-001 | Show/Hide Browser toggle, with the Browser Config Menu (a small menu arrow) beside it | x 9 to 35 | Always present. The menu holds Full-Height Browser, Groove Pool, and Tuning | obs, man 3.1 |
| L-CTRL-002 | Link toggle | In the tempo group, before the tempo controls | Hidden by default; shown when Link is enabled in Settings | man 3.1, 37 |
| L-CTRL-003 | Tempo Follower toggle | In the tempo group | Hidden by default; Settings, Tempo & MIDI: Show Tempo Follower Toggle | man 3.1, obs (setting) |
| L-CTRL-004 | Tap tempo button | x 54, 28 pt wide | Always present | obs |
| L-CTRL-005 | Tempo field | x 82 to 126 | Always present | obs |
| L-CTRL-006 | Phase Nudge Down and Up buttons | x 126 and 145, 18 pt each | Always present | obs |
| L-CTRL-007 | Time signature numerator and denominator fields | x 167 and 189. Two separate fields: click either one, type a number, and press Return; each field's edit is its own undo step | Always present | obs |
| L-CTRL-008 | Global Groove Amount field | x 209 to 249; everything to its right moves 40 pt right | Shown only while the Groove Pool holds a groove | obs |
| L-CTRL-009 | Metronome toggle, with the Metronome Settings menu arrow | x 209 and 237 (without L-CTRL-008) | Always present | obs, man 19.4.1 |
| L-CTRL-010 | Quantization Menu (global launch quantisation) | x 250 to 295. Choices, in order: None, 8 Bars, 4 Bars, 2 Bars, 1 Bar (the default), 1/2, 1/2T, 1/4, 1/4T, 1/8, 1/8T, 1/16, 1/16T, 1/32 | Always present | obs; choices obs (Live 12, 15 October 2026) |
| L-CTRL-011 | Scale Mode toggle, Root Note chooser, Scale Name chooser | x 314, 335, 381 to 452 | Always present; reflects the selected clip | obs, man 3.13 |
| L-CTRL-012 | Follow switch | x 555 | Always present | obs |
| L-CTRL-013 | Arrangement Position fields (bars, beats, sixteenths) | x 582 to 659. Typing a position such as 3.3.1 moves the insert marker exactly there | Always present | obs |
| L-CTRL-014 | Play, Stop, Arrangement Record buttons | x 678, 701, 725 (18 pt each) | Always present | obs |
| L-CTRL-015 | MIDI Arrangement Overdub, Automation Arm, Re-Enable Automation, Capture MIDI, Session Record | x 758, 778, 796, 816, 834 | Always present | obs |
| L-CTRL-016 | Loop Start fields (bars, beats, sixteenths) | x 875 to 953 | Always present | obs |
| L-CTRL-017 | Punch-In switch, Arrangement Loop switch, Punch-Out switch | x 956, 979, 1002 | Always present | obs |
| L-CTRL-018 | Loop Length fields | x 1029 to 1106 | Always present | obs |
| L-CTRL-019 | Draw Mode switch | x 1283 | Always present | obs |
| L-CTRL-020 | Computer MIDI Keyboard switch | x 1302 | Always present | obs |
| L-CTRL-021 | Key Map Mode switch ("Key"), with key input and output indicators | x 1321; indicators at 1350 | Always present | obs |
| L-CTRL-022 | MIDI Map Mode switch ("MIDI") | x 1359 | Always present | obs |
| L-CTRL-023 | Sample rate display (opens the Audio settings) | x 1389 to 1439 | Always present | obs |
| L-CTRL-024 | CPU Meter, with a menu arrow | x 1439 to 1496; reads OFF while the audio engine is off | Always present | obs |
| L-CTRL-025 | Overload (disk) indicator | x 1498 | Always present | obs |
| L-CTRL-026 | MIDI track input and output indicators | x 1527 | Always present | obs |
| L-CTRL-027 | View selector: Session View button, then Arrangement View button | x 1549 to 1590, at the right end | Always present; click to switch views (observed); S-VIEW-003 | obs, man 3.6 |
| D24 | Assistant toggle (Show/Hide Assistant) | Beatlayer addition. The Control Bar's last control, x 1572 to 1590 (18 pt), right of the Arrangement View button. To make room, Beatlayer draws L-CTRL-019 to L-CTRL-027 20 pt (the toggle and a 2 pt gap) left of Live's positions above: Draw Mode 1263, Computer MIDI Keyboard 1282, Key Map 1301 (indicators 1330), MIDI Map 1339, Sample Rate 1369 to 1419, CPU 1419 to 1476, Overload 1478, MIDI indicators 1507, view selector 1529 to 1570. The stretching gap before Draw Mode is 157 pt instead of 177, and the smallest content width is 1340 pt instead of 1320 | Always present; opens and closes the assistant pane | Beatlayer addition, decided 16 October 2026 (D24) |

## Browser (L-BRW)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-BRW-001 | Top row: Browse Back, Browse Forward, search field, Show Filter View toggle, Filter View Menu | y 61 to 82. Back at x 9, Forward at 29, search field 50 to 270 (220 pt), filter toggle at 275, menu at 297 | Always present with the browser | obs, man 4 |
| L-BRW-002 | Sidebar | x 9 to 109 (100 pt), y 84 to 766. Sections in order: Collections, Library, Places | Always present with the browser | obs, man 4 |
| L-BRW-003 | Collections section | Heading, then Favorites and the other collection labels (seven colours in all) | Always present | obs, man 4.5 |
| L-BRW-004 | Library section labels | In order: All, Sounds, Drums, Instruments, Audio Effects, MIDI Effects, Modulators, Max for Live, Plug-Ins, Clips, Samples, Grooves, Tunings, Templates; then any saved custom labels | Always present | obs, man 4.6 |
| L-BRW-005 | Places section labels | In order: Packs, Splice, Cloud, Push, User Library, Current Project, then added folders, then Add Folder... | Splice, Cloud and Push can be hidden in Settings, Library | obs, man 4.7 |
| L-BRW-006 | Content pane | x 110 to 311 (201 pt), right of the sidebar; the divider between them can be dragged | Always present with the browser | obs, man 4.1 |
| L-BRW-007 | Filter View (filter groups and their tags) | Top of the content pane, y 84 to 204 in the default state, under a group heading ("Sounds") with tag buttons that wrap | Show Filter View toggle (L-BRW-001), S-BRW-013 | obs, man 4.4 |
| L-BRW-008 | Results bar (Clear, Add Label) | Between the filters and the list while searching or filtering | Appears with a search or filter | man 4.2.1 |
| L-BRW-009 | Column header row (Name, plus optional columns) with the Content Options menu | y 208 to 227 | Always present | obs, man 4.1 |
| L-BRW-010 | Item list | y 227 to 744 | Always present | obs |
| L-BRW-011 | Browser File Preview toggle and preview strip (waveform of the selected file) | Bottom-left of the content pane, toggle at x 114, y 748 | Always present; M-OPT-006 | obs, man 4.9 |
| L-BRW-012 | Quick Tags section | In the content pane when enabled from the Filter View Menu | Filter View Menu | man 4.4.4 |
| L-BRW-013 | Tag Editor | Opens beside the content pane, with a close button at its top right | Filter View Menu, S-BRW-014 | man 4.4.3 |
| L-BRW-014 | Tuning section | In the browser column, hidden by default | Browser Config Menu (L-CTRL-001), M-VIEW-023 | man 15.2 |
| L-BRW-015 | Hot-swap bar | An orange bar along the top of the browser reading "Swapping Audio Effect" for an audio effect, with a close (×) button. The browser jumps to the device's category (Audio Effects) and opens the device's own folder of presets beneath its entry | While hot-swapping: the Hot-Swap Presets button on a device title bar (L-DEV-004) is on; the close button ends it | obs (Live 12.4.6, 12 October 2026) |

## Session View (L-SES)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-SES-001 | Track columns | From x 320, each 94 pt wide by default (resizable by dragging a title bar's edge; Option/Alt resizes all). Regular tracks, then a drop area, then return tracks, then the Main track at the right edge | Always present in the Session View | obs, man 7.2 |
| L-SES-002 | Track title bars | Top row, y 61 to 78 (17 pt), in the track's colour, name left-aligned | Always present | obs |
| L-SES-003 | Clip slots | Under the title bars, one row per scene, 18 pt per row; a slot shows a launch triangle or a stop square at its left | Always present | obs, man 7.1 |
| L-SES-004 | Drop area ("Drop Files and Devices Here") | Between the last regular track and the first return track (x 700 to 1297 by default); stretches with the window | Always present | obs, man 18.2 |
| L-SES-005 | Return track columns | Left of the Main track, 94 pt each (1298 and 1393 by default); their slots can't hold clips | Shown by M-VIEW-032g, S-TRACK-011 | obs, man 18.4 |
| L-SES-006 | Main track column | Right edge, 1487 to 1591 (104 pt). Holds scene launch buttons at its left, scene names, and scene numbers at its right edge. A new set has 8 scenes, none with a name of its own: each shows its launch triangle at the left and its number at the right, and the numbers are positions (inserting a scene above renumbers the ones below) | Always present | obs, man 7.2; obs (Live 12, 15 October 2026) |
| L-SES-007 | Scene tempo and time signature fields | Revealed at the left of the Main track's column by dragging its left edge | Hidden by default; M-VIEW-035, the scene context menu (C-SCENE-013) | obs (menus), man 7.2.1 |
| L-SES-008 | Track status row (clip stop buttons and status fields) | Under the slots and above the mixer, 17 pt tall. Each track's stop button at its left, its status display beside it; the Main track has Stop All Clips (a square) at its left and, at its right, the global follow-action switch (Enable Follow Actions Globally) and the Back to Arrangement button. A track's stop button turns green while a stop is queued | Always present | obs, man 7.3; stop button and Main's switches obs (Live 12, 15 October 2026) |
| L-SES-009 | Mixer (Session) | Under the status row to the bottom of the main view; the slot area takes the remaining height. Sections from top: In/Out, Sends, Returns, Volume (mixer strip), Track Options, Crossfader, Performance Impact (see L-MIX) | Mixer toggle in the Status Bar (L-STAT-008), M-VIEW-031 | obs, man 18.1 |
| L-SES-010 | Group slots | In a group track's column, showing launch or stop buttons for the grouped clips | With group tracks | man 7.1 |
| L-SES-011 | Scene View | In the detail area, when a scene or the Main track title is selected: title bar, then tempo and signature, then follow-action controls. The scene's number, its Tempo and Signature fields, and its Follow Action section (the Follow Action button; actions A and B, a new scene's A "Next" and B "No Action"; chances 100 % and 0 %; a Longest button where a clip is linked; a follow time "1x" with Longest on, or Unlinked and a bars.beats.sixteenths time, 1.0.0 by default, with it off; observed again in Live 12.4.3 on 17 October 2026, when the default time read 1.0.0 and a three-bar longest clip read "1x (3.0.0)"). The Tempo and Signature fields are grey, showing the set's current values, until the scene is given its own, then drawn active | Selecting a scene; double-clicking its number (I-CLICK-036) | man 7.2.2; obs (Live 12, 15 October 2026) |
| L-SES-012 | Back to Arrangement button | Right end of the Main track's status row | Lights when a track plays Session clips over the Arrangement | obs, man 3.7 |
| L-SES-013 | Clip slot states | An empty slot on an audio or MIDI track shows a small stop square; on an armed track the empty slots show record circles instead and the track's arm button is red; return tracks' slots are blank. A slot with a clip is filled with the clip's colour (the track's colour by default), with its launch triangle at the left: grey while stopped, flashing white and green while queued, green while playing | Always present | obs (Live 12, 15 October 2026) |
| L-SES-014 | Track status field while its clip plays | A number at the left (counting up as the clip loops), a progress circle, and the clip's length in beats at the right (3 for a one-bar 3/4 clip, 4 for 4/4) | While a clip plays | obs (Live 12, 15 October 2026) |

## Arrangement View (L-ARR)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-ARR-001 | Overview | Top strip, y 61 to 74, across the main view | M-VIEW-034, S-VIEW-011 | obs, man 6.1 |
| L-ARR-002 | Beat-time ruler (bars, beats) with the loop brace drawn in it | y 81 to 99. After a time-signature change its bar labels count from the change (a change at bar 4 gives 4, 8, 12, ...) | Always present | obs, man 6.1 |
| L-ARR-003 | Scrub area with locators | y 99 to 115 | Always present | obs, man 6.1, 6.4 |
| L-ARR-004 | Time signature marker lane | Just under the beat-time ruler. A fragmentary bar before a change is drawn hatched in this lane | Only when the set has signature changes; Create > Insert Time Signature Change (M-CREATE-017) adds one at the insert marker | obs, man 6.5 |
| L-ARR-005 | Track lanes | From y 119 down, x 320 to 1295; default height of a new track 88 pt. Group tracks show a summary of their clips | Always present | obs, man 6.1 |
| L-ARR-006 | Track header column (Arrangement track controls) | Right of the lanes, x 1297 to 1591 (294 pt, 18% of the width): title 1297 to 1400, In/Out 1403 to 1476, mixer 1479 to 1577, meter 1580 to 1591 | Always present; parts set by M-VIEW-033 | obs, man 6.1 |
| L-ARR-007 | Track title (fold triangle, then the name) | First column of each header, filled with the track's colour. Each track also has its own Back to Arrangement button | Always present | obs, man 3.7 |
| L-ARR-008 | In/Out column: input type, input channel, monitoring (In, Auto, Off), output type, output channel | Second column, one row each | M-VIEW-033a | obs |
| L-ARR-009 | Mixer column: row 1 track activator (number), Solo, Arm; row 2 volume, pan; row 3 sends A, B | Third column | M-VIEW-033b, 033c | obs |
| L-ARR-010 | Controls above the headers: Set Locator, Automation Mode, Lock Envelopes; Previous and Next Locator under Set | y 82 (row 1) and 100 (row 2), from x 1299 | Always present | obs, man 6.1 |
| L-ARR-011 | Automation lanes | Under each track's main lane | Automation Mode (S-ENV-001, M-VIEW-036) | man 26.5.2 |
| L-ARR-012 | Take lanes | Under the main lane | S-COMP-001, track context menu | man 21.1 |
| L-ARR-013 | Drop area ("Drop Files and Devices Here") | Under the last track | Always present | obs, man 6.1 |
| L-ARR-014 | Return tracks and Main track | Pinned to the bottom of the lanes, one 18 pt row each (A at 688, B at 706, Main at 725), with the same header columns | Returns: M-VIEW-033d, S-TRACK-011 | obs |
| L-ARR-015 | Grid spacing indicator (for example "1/1") | Lower right of the lanes, x 1265 to 1293, y 734 | Always present | obs, man 6.10 |
| L-ARR-016 | Time ruler (minutes and seconds) | Under the lanes, y 746 to 760 | Always present | obs, man 6.1 |
| L-ARR-017 | Waveform zoom toggle and Waveform Vertical Zoom Level field, then Optimise Height (H) and Optimise Width (W) | Under the header column, y 748: x 1303, 1314 to 1351, 1372, 1390 | Always present | obs, man 6.1 |
| L-ARR-018 | Mixer in the Arrangement | The Session mixer's columns (same widths and x positions, returns and Main at the right) and sections under the lanes, with a row of track title bars (about 17 pt, each track's colour and name) along its top. Its sections start at the height the Session mixer's do, so its height is the sections' height plus the title row; the lanes, the pinned returns and Main, and the time ruler stay above it (observed in 12.4.6, 11 October 2026) | Mixer toggle (L-STAT-008), M-VIEW-031, S-VIEW-014; one switch per view: hidden in a new set (observed in 12.4.6) | obs, man 6.15 |
| L-ARR-019 | Back to Arrangement button | Top right of the scrub area | Lights when Session clips override the Arrangement | man 3.7 |

## Mixer sections (L-MIX)

The Session mixer and the Arrangement's mixer share these sections, top to bottom. Heights are at 1600 × 1000 in the default state.

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-MIX-001 | In/Out section: "From" label, input type, input channel, Monitor label with In, Auto, Off, "To" label, output type, output channel | 115 pt tall; return tracks show only "Audio To"; the Main track shows Cue Out (1/2) above Main Out (1/2) | M-VIEW-032a, S-VIEW-012 | obs, man 17; Main's two rows obs (Live 12, 15 October 2026) |
| L-MIX-002 | Sends section: one knob per return track, staggered in two rows, labelled A, B | 56 pt tall; return tracks show their own sends dimmed; the Main track shows the returns' Pre/Post switches, which read Post in a new set | M-VIEW-032b, S-VIEW-013 | obs; switches obs (Live 12.4.6, 12 October 2026) |
| L-MIX-003 | Mixer strip: the peak level readout at the top left (x 8 to 42, y 13 to 24 from the section's top-left corner; it shows −∞ with nothing playing and isn't the volume), the pan knob under it (centred at 25, 46, about 25 pt across, with its reset triangle above at y about 30), the activator (number), Solo, and Arm in a column at the left (x 15 to 36; y 64 to 85, 91 to 102.5, 106 to 119), the fader's handle at x 46 to 59 left of the 11-pt meter (x 60 to 71, y 11 to 119), and the meter's scale (0, 12, 24, 36, 48, 60 at x 74 to 86, y 23.5, 44.5, 64.5, 84.5, 99, 111), where the handle sits at those values. No volume number field at the default height. Return tracks show their letter on the activator and have Solo but no Arm; group tracks have the activator and Solo but no Arm; MIDI tracks without an instrument show only a dotted MIDI meter and the switches; a deactivated activator shows its number on grey (observed in 12.4.6, 11 October 2026) | 123 pt tall | M-VIEW-032c | obs, man 18.1 |
| L-MIX-004 | Main track strip: volume, pan, cue volume knob (Preview/Cue Volume: blue, with a headphone icon), and the global Solo/Cue mode button (labelled Solo). No Track Activator and no track Solo | In the Main track's column | Always present | obs |
| L-MIX-005 | Track Options (track delay, Keep Monitoring Latency) | Under the strip | M-VIEW-032d (hidden by default) | man 18.7, 18.8, 41.4.1.2 |
| L-MIX-006 | Crossfader and per-track A/B assign buttons | Under the strip; the crossfader in the Main track. Each audio track and return track gets A and B assign buttons below its strip, both off by default; a MIDI track with no instrument gets none. Main gets the crossfader, a horizontal slider, centred by default | M-VIEW-032e (hidden by default) | man 18.5; obs (Live 12, 15 October 2026) |
| L-MIX-007 | Performance Impact indicators | Under the strip | M-VIEW-032f (hidden by default) | man 18.9 |
| L-MIX-008 | Extended meters (tick marks, numeric volume, peak hold) | Appear when the mixer is dragged taller | Mixer height | man 18.1.1 |
| L-MIX-009 | Track colour strip | Bottom edge of each column, under the strip, about 6.5 pt tall (observed in 12.4.6, 11 October 2026) | Always present | obs |

## Clip View (L-CLIP)

Measured with an empty MIDI clip selected in the Session View, the Learn View closed.

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-CLIP-001 | Clip View | Detail area right of the Info View, x 249 to 1591, y 738 to 963 | Clip View toggle (L-STAT-004), double-clicking a clip, M-VIEW-010, S-VIEW-015 | obs, man 8 |
| L-CLIP-002 | Clip title bar: Clip Activator, clip name in the clip's colour; Save Default Clip button on audio clips | y 741 to 755 over the panels, x 251 to 590 | Always present in the Clip View; double-click folds the panels | obs, man 8.1.1 |
| L-CLIP-003 | Panel selector row: Main Properties, Launch Properties (left group); Pitch and Time Utilities, MIDI Transformation Tools, MIDI Generative Tools (right group). Audio clips show Audio Utilities and audio transformation panels in the right group | y 757 to 773, x 249 to 591 | Always present | obs, man 8.1.2 |
| L-CLIP-004 | Main Properties panel: Start and End (each with Show and Set), Duplicate, Loop switch, Position and Length (each with Show and Set), Signature numerator and denominator, Groove chooser with Hot-Swap and Commit, Scale Mode with root and scale. Lengths count from 0 (a one-bar clip's Length reads 1. 0. 0, a three-bar clip's 3. 0. 0); Start takes positions before 1.1.1 (typing 0.3.1 gives -1. 3. 1) | x 251 to 417 (166 pt) | Panel selector | obs, man 8.2 |
| L-CLIP-005 | Launch Properties panel: launch mode, legato, quantisation, velocity, follow actions; MIDI bank and program on MIDI clips. In order: Follow Action (an on/off button); actions A and B (the list, each with an icon: No Action, Stop, Play Again, Previous, Next, First, Last, Any, Other, Jump; A is Next and B is No Action by default); chances A 100 % and B 0 %; Linked (on by default) with a multiplier "1x", which becomes a bars.beats.sixteenths field (1.0.0 by default, one bar; observed 17 October 2026) when switched to Unlinked, and reads "Clip End" while the clip's Loop is off; launch mode (Trigger by default, Gate, Toggle, Repeat); a Legato switch; two nudge buttons («, »); Quantize (Global by default, then the global menu's list, None to 1/32); Velocity (0.0 % by default); on MIDI clips, Bank, Sub and Pgm fields, empty ("---") by default | Second panel in the left group, Session clips only | Panel selector | man 8.3; obs (Live 12, 15 October 2026) |
| L-CLIP-006 | Pitch and Time Utilities panel: scale/chord display with Transpose, Fit to Scale, Invert; interval and Add Interval; Stretch knob, ×2, /2; Grid and Set Length; Humanize amount and Humanize; Reverse, Legato | x 424 to 588 (164 pt) | Panel selector | obs, man 8.5 |
| L-CLIP-007 | MIDI Transformation and Generative Tools panels | Same place as L-CLIP-006 | Panel selector | man 8.6 |
| L-CLIP-008 | Audio Utilities panel: Warp switch and mode, Reverse, Edit, fades, RAM, high quality, gain, transpose and detune | Right panel group for audio clips | Panel selector | man 8.4 |
| L-CLIP-009 | Panel arrangement: horizontal (panels side by side) or vertical (stacked) | Dragging the panels' right edge, or M-VIEW-015 to 017 | | man 8.1.2 |
| L-CLIP-010 | Editor toolbar: Find and Select Notes (magnifier), Fold, Scale (fold to scale), Highlight Scale; then the Notes, Envelopes, MPE tabs at the right; then the grid chooser | y 741 to 755, x 593 to 1591; tabs at 1029, 1090, 1151; grid at 1563 | Always present in the MIDI Note Editor | obs, man 10.1 |
| L-CLIP-011 | Editor ruler and loop/marker strip | Ruler y 760 to 774; markers and loop bar 779 to 795. Before 1.1.1 the ruler reads -1.3, -1.4, then 1, 1.2: there's no bar 0 | Always present | obs |
| L-CLIP-012 | Piano ruler (keys) with the preview switch above it | x 622 to 649, from 812 down; preview at 632, 804 | Always present | obs, man 10.1 |
| L-CLIP-013 | Note grid | Right of the keys to the window edge | Always present | obs |
| L-CLIP-014 | Velocity lane (and chance lane) | y 904 to 947, labelled 127, 64, 1 | Lane header context menu swaps velocity and chance | obs, man 10.5.12 |
| L-CLIP-015 | Editor footer: MIDI tool menu, then Velocity with Randomize and value, Ramp with two values, Deviation | y 948 to 961 | Always present | obs |
| L-CLIP-016 | Sample Editor (audio clips) | Same place as the MIDI Note Editor | Content Editor tab (M-VIEW-018) | man 8, 9 |
| L-CLIP-017 | Envelope Editor | Same place, with device and parameter choosers. Its bottom row holds a device chooser, a control chooser, and a Linked switch (on by default). For a MIDI clip on a MIDI track with no devices, the device chooser offers "MIDI Ctrl", "Mixer" and "Only show adjusted envelopes"; the control chooser for Mixer offers only "Speaker On" and the same filter. With no envelope drawn, the lane shows a dashed red line | Envelopes tab (M-VIEW-019, S-CLIPMODE-004) | man 27.1; obs (Live 12, 15 October 2026) |
| L-CLIP-018 | MPE (Expression) Editor | Same place | MPE tab (M-VIEW-020, S-CLIPMODE-005) | man 12.1 |
| L-CLIP-019 | Note filters (Find and Select Notes) | Above the note grid | Magnifier button, S-NOTE-037 | man 10.5.3 |

## Device View (L-DEV)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-DEV-001 | Device View | Detail area right of the Info View, x 249 to 1591, y 774 to 962 for an empty chain ("Drop an Instrument or Sample Here" centred) | Device View toggle (L-STAT-006), double-clicking a track title bar, M-VIEW-011, S-VIEW-016 | obs, man 23.1 |
| L-DEV-002 | Stacked Clip and Device View | Clip View above, Device View below, sharing the detail area | Both toggles on, or Option/Alt-click a toggle (S-VIEW-005) | man 3.12, 23.1 |
| L-DEV-003 | Device chain | Devices left to right in signal order, about 7 pt apart (observed 12 October 2026), with level meters between them; MIDI effects, then the instrument, then audio effects. Each device is its own width | | obs, man 23.2 |
| L-DEV-004 | Device title bar, about 16 pt tall: the Device Activator (round, lit while on) at the left, then the expand toggle where the device has an expanded view, then the name (the Device Title Bar: click selects, drag moves); at the right, Hot-Swap Presets, Save Preset, and Show Options, round buttons in that order (observed 12 October 2026). The name is the preset's or the given name while Show Preset Name is on (the default) and the device's own name when it is off. The selected device's title bar is light, others dark grey | Top of each device | | obs, man 23.2.1 |
| L-DEV-005 | A/B indicator ("(B)" after the name) | In the title bar | When the B state is active | man 23.2.2 |
| L-DEV-006 | Folded device | An upright strip a title bar wide, the title bar on its side: the activator at the top, the name reading upwards, Hot-Swap Presets, Save Preset, and Show Options at the bottom (observed 12 October 2026) | Double-click the title bar (and the strip to unfold), or Fold in its context menu (C-DEVICE-008, S-DEV-014) | obs, man 23.1 |
| L-DEV-007 | Expanded view (above the Device View) | Above the device, for devices that have one | Arrow toggle in the title bar | man 23.2.1 |
| L-DEV-008 | Rack: macro controls, chain list, zones, chain devices; Drum Rack pad view | Inside the rack, left to right | Each view by its selector in the column of view selectors down the rack's left edge, under its title bar; the chain list also by M-VIEW-041 and 042 (S-DEV-012, S-DEV-013) | man 25.3 |
| L-DEV-009 | Show Options toggle | Right end of each title bar; opens the device's context menu (C-DEVICE) | | man 23.2.1 |
| L-DEV-010 | Plug-in device with its window button | In the chain; the plug-in's own editor opens in a separate window (L-OTH-006). The window button (a wrench, named Show/Hide Plug-In Window in the Info View) sits after the activator and the fold triangle and before the name. The device panel has an X-Y control with two parameter choosers, both "none" by default | S-DEV-009 | man 24.1; obs (Live 12, 12 October 2026) |
| L-DEV-011 | Device switched off | The activator turns dark and the device's coloured highlights (knob arcs, lit buttons, number boxes' fill, a display's curve) turn grey; labels and value text stay white, and the controls stay visible and usable | The Device Activator | obs (Live 12.4.6, 12 October 2026) |
| L-DEV-012 | Sidechain section (Compressor) | A strip down the device's left edge, labelled "Sidechain" up the strip, with its show/hide toggle at the top and Sidechain Listen (a headphones button) under it. The section shows, top to bottom: External (a toggle, off in a new device); Source (a label) and its chooser, No Input by default; the source's channel chooser (empty while the source is No Input); Mix (a number box, 100 %); SC Gain (a number box, 0.00 dB). Mix and SC Gain light up when External is on. Compressor's SC Filter column (SC Filter toggle, Type, Freq, Res) sits beside it and belongs to the device | The strip's toggle | obs (Live 12.4.6, 12 October 2026) |
| L-DEV-013 | Device controls as drawn | Toggles are rectangular text buttons, orange with dark text when on and dark grey with light text when off. Number boxes are dark with the value centred, and when lit a blue fill shows the value's place in its range. Choosers are dark boxes with the value and a ▼ at the right. A display cell is black with a thin border, with rows of controls inside it along its top and bottom edges (Compressor: Thresh, GR/Output and Out at the top; view buttons, Knee, Look. and Env. at the bottom). Values read in three significant figures (1.00 ms, 30.0 ms, 0 ms, 4.00 : 1, 80.0 Hz, 1.24 kHz, 10.0 kHz, 0.00 dB, −4.9 dB, 6.0 dB, 100 %, 21 %, 0.0 %, 0.71) | Inside each device | obs (Live 12.4.6, 12 October 2026) |
| L-DEV-014 | Display views and drawn displays | A device's display cell draws its own data in the cell's black frame. Squeeze's display (a Beatlayer reading of Live's Compressor): a gain reduction meter down the cell's right edge, and three small view buttons (Curve, Act, Min) along the cell's top-right corner, choosing the transfer curve, the activity, or the meter alone. Contour's display: the spectrum behind the curves while Analyze is on, a curve in the accent per curve in use, and a dot per switched-on band. Where Live puts the view buttons and what each view draws beyond the manual is to observe | Inside the device, in its display cell | Beatlayer (WP-5.43); man 29.9 |
| L-DEV-015 | Eight-slot drum sampler panel (Beatlayer's own look, not Live's) | Eight slot cells in a row along the top, each 64 pt wide and 36 pt tall at 100% before the panel grows, with the slot's number above its key and a bar along its foot while it holds a sample; the selected slot's cell is framed in the accent. Below the cells, the controls of the selected slot in columns: Start, Transpose with its Velocity and Random amounts, Stretch with its Mode and Velocity amount, the filter's switch, Type, Freq, Res, Velocity and Random, the saturator's switch and Drive, Decay with its mode, Pan with its Velocity and Random, Volume with its Velocity, Mute and Solo. At the right, the global Volume, Transpose, Time and Link. A sample dropped on a cell loads into that slot | Click or Enter on a cell selects its slot (`device.select_slot`); Delete on a cell clears its sample | Beatlayer's own look (T156); Live's layout is to observe |
| L-DEV-016 | Modulator panels | Wave Mod: its waveform chooser, Shape, Steps, Jitter and Smooth, its display, the rate controls (Time Mode, Rate, ×10, Beat Rate, Phase, Offset, Hold, R), slot 1's mapping cell with its Amount, Min and Max, and Mod, Polarity and Depth; Curve Mod: its trigger controls, the shape display with Clear and six preset buttons along its top and Grid and Snap along its bottom, the rate controls and the same mapping cell and output controls. Each mapping cell shows its target, a Map button that lights while armed and Unmap; the expand toggle adds the other seven cells (Multimap). While a Map button is lit, the parameters it can map are tinted. Beatlayer's own look, as a Live user finds LFO and Shaper | Inside the device | Beatlayer (WP-5.10); man 33.2.3, 33.2.4 |
| L-DEV-017 | Multisampler's tabs (Beatlayer's Mosaic; not yet observed in Live's Sampler) | Six small text buttons at the right of the title bar, left of Hot-Swap Presets: Zone (shows the zone editor, lit while it shows), then Sample, Pitch/Osc, Filter/Global, Modulation and MIDI (the one lit is the tab whose controls fill the body) | Always, on a multisampler | man 31.10.4; the placement is Beatlayer's own, to observe |
| L-DEV-018 | Multisampler's zone editor (not yet observed in Live) | A view above the Device View, over the foot of the main view, as wide as the detail area and 150, 230 or 330 pt tall (Small, Medium, Large), its bottom 4 pt above the detail area. A 22 pt header holds Hot-Swap Sample, Auto, Lin, Pow, RR and Key, Vel, Sel; under it, a 180 pt layer list at the left, and the editor right of it with a 14 pt scale (C-2 to G8 for keys, 1 to 127, 0 to 127) over one row per layer, 10, 16 or 26 pt tall, its zone bar drawn with a fade handle at each top corner. The select editor's selector indicator sits in the scale | The Zone tab (L-DEV-015) | man 31.10.5; sizes and placement are Beatlayer's own, to observe |
| L-DEV-019 | Multisampler's Sample tab (not yet observed in Live) | The body: a waveform display across its top with the Sample chooser, B, M, L and R along its top edge and the markers drawn over it; under it two rows of fields: Root, Detune, Volume, Pan, Reverse, Interpolation, Snap and RAM, then Start, End, Loop Start, Loop End, Link and the Sustain and Release Mode choosers | The Sample tab (L-DEV-015) | man 31.10.6; the order and sizes are Beatlayer's own, to observe |

## Groove Pool (L-GRV)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-GRV-001 | Groove Pool | Lower part of the browser column, under the browser, which shrinks to make room: browser 61 to 392, Groove Pool 400 to 729 (about half each) | Browser Config Menu, M-VIEW-022, S-VIEW-017 (observed) | obs, man 14.1 |
| L-GRV-002 | Column headers: Groove Name, Base, Quantize, Timing, Random, Velocity | Top row of the pool; columns past the column's width are cut off | | obs, man 14.1.1 |
| L-GRV-003 | Groove row: name, then hot-swap, save and commit buttons, then the parameter fields | One row per groove | | obs, man 14.1 |
| L-GRV-004 | Empty-pool text ("Drop Clips or Grooves Here") | Centred in the pool | | obs |
| L-GRV-005 | Footer: section name at the left, Global Amount at the right | Bottom row of the pool | | obs, man 14.1.1 |

## Info View (L-INFO)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-INFO-001 | Info View | Bottom left, x 9 to 240 (231 pt), the same height as the detail area | Info toggle at the left of the Status Bar (L-STAT-001), M-VIEW-004, S-VIEW-008 | obs, man 2.2.2 |
| L-INFO-002 | Contents: the hovered element's name in bold, then a description, then its shortcut on a separate line | Text wraps to the panel's width | Updates as the pointer moves | obs |
| L-INFO-003 | User info text | Same place, editable | Edit Info Text (M-EDIT-025) | man 2.2.2 |

## Learn View (L-LRN)

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-LRN-001 | Learn View | Right column, x 1296 to 1591 (295 pt), y 61 to 988 | M-VIEW-005, M-HELP-001, S-VIEW-018 (observed); open on a first launch | obs, man 2.2.1 |
| L-LRN-002 | Header row: close button at the left, title, then Home, Back, Forward at the right | y 61 to 82 | | obs |
| L-LRN-003 | Home page: introduction, topic filter buttons, then modules with lesson lists | Scrolls | | obs, man 2.2.1 |
| L-LRN-004 | Lesson page: video player, text, completion button, links | Scrolls | | man 2.2.1 |
| L-LRN-005 | Picture-in-picture video window | Separate floating window that stays on top | Button beside the video's progress bar | man 2.2.1 |

Beatlayer's lessons are its own content (parity matrix: M10); this panel's position and controls are what D16 covers.

## Status Bar (L-STAT)

Left to right, at 1600 × 1000 with the Learn View closed.

| ID | Element | Placement at 1600 × 1000 | Shown or hidden by | Source |
|---|---|---|---|---|
| L-STAT-001 | Info View toggle ("i") | x 17 | Always present | obs, man 2.2.2 |
| L-STAT-002 | Message area: errors, updates, missing files, background analysis (clickable; for example the engine-off message opens the Audio settings) | x 44 to about 1240, stretching | Always present | obs, man 3.2 |
| L-STAT-003 | Note and marker readout | In the message area while editing notes or hovering an insert marker | | man 3.2 |
| L-STAT-004 | Clip View toggle (triangle) | x 1259 | Always present | obs, man 3.12 |
| L-STAT-005 | Clip View selector (selected clip's name) | x 1270 to 1468 | Always present | obs |
| L-STAT-006 | Device View toggle (triangle) | x 1486 | Always present | obs |
| L-STAT-007 | Device View selector (selected track's name); right-click lists the track's devices | x 1497 to 1538 | Always present; S-DEV-005 | obs, man 23.1 |
| L-STAT-008 | Mixer View toggle, with the Mixer Config Menu arrow | x 1557 to 1591 | Always present; the menu holds the Mixer Controls choices | obs, man 18.1 |

The active view's toggle is drawn on (filled) with an upward triangle; the inactive one shows a sideways triangle.

## Second window (L-WIN2)

| ID | Element | Placement | Shown or hidden by | Source |
|---|---|---|---|---|
| L-WIN2-001 | Second window | Separate window, 990 × 703 pt when first opened. Shows the main view the first window isn't showing (the Arrangement when the main window shows the Session) | M-VIEW-002, S-VIEW-002 (observed) | obs, man 7.5 |
| L-WIN2-002 | Its contents | Its own Control Bar (the same controls, cut off at the window's width), the main view, a detail area with the Clip or Device View, and its own Status Bar. No browser by default | | obs |
| L-WIN2-003 | Moving the detail view between windows | The Clip or Device View opens in the focused window; F12 (or Shift+Tab) moves it to the other window | S-VIEW-004 | man 8 |

## Settings window (L-SET)

| ID | Element | Placement | Shown or hidden by | Source |
|---|---|---|---|---|
| L-SET-001 | Settings window | Separate window, 562 × 617 pt; reopens on the page last shown | M-APP-002 (Options menu on Windows), S-VIEW-019, clicking the sample rate or an audio message | obs, man 2.3 |
| L-SET-002 | Page chooser | Left column, 140 pt wide, one 25 pt row per page from y 34. Pages in order: Display & Input, Theme & Colors, Audio, Link, Tempo & MIDI, File & Folder, Library, Plug-Ins, Agents, Record, Warp & Launch, Licenses & Updates (Agents is a Beatlayer addition, decided 14 October 2026) | | obs, man 2.3 |
| L-SET-003 | Page layout | Section headings with a rule; labels at x 154, controls at x 354; switches 54 pt wide, choosers 108 pt (audio device choosers 193 pt); rows about 22 pt apart | | obs |
| L-SET-010 | Display & Input | Display: Language, Zoom. Display Options: Outline View in Focus (On/Off switch, On), Show Scroll Bars (chooser, Follow System), Follow Behavior (a button showing Scroll), Hide Labels (a button showing Show). Navigation and Keyboard: Use Tab Key to Move Focus (Off), Wrap Tab Navigation (On), Move Clips With Arrow Keys (On), all On/Off switches; the values are this install's, observed in 12.4.6 on 11 October 2026. Mouse and Pen Input: Pen Tablet Mode, Permanent Scrub Areas, Draw MIDI Notes with Pitch Lock. Application Options: Don't Show Again Warnings (Restore) | | obs, man 2.3.1 |
| L-SET-011 | Theme & Colors | Theme: Theme, Appearance, Tone, High Contrast. Customization: Grid Line Intensity, Brightness, Color Intensity, Color Hue. Track and Clip Colors: Auto-Assign Track Colors, Default Track Color, Reduced Automatic Colors, Clip Color | | obs, man 2.3.2 |
| L-SET-012 | Audio | Audio Device: Driver Type, Audio Input Device, Audio Output Device; then, with an active device, Input Config and Output Config, sample rate, latency (buffer size; Driver Error Compensation, a number box from -1000.00 to 1000.00 ms, Beatlayer's bound; and an Overall Latency row: input + output latency + the compensation), and the test section: Test Tone (a switch), Tone Volume, and Tone Frequency (number boxes; D23). Beatlayer uses -36 dBFS and 440 Hz, -60 to 0 dBFS and 20 Hz to 20 kHz, and 54 pt number boxes | | obs (device section), man 2.3.3, 38 |
| L-SET-013 | Link | Link and Link Audio settings (shows only a message while the audio driver can't use Link) | | obs (message), man 2.3.4 |
| L-SET-014 | Tempo & MIDI | Tempo Follower: Show Tempo Follower Toggle, Input Channel. A second section: Show Resync Button. MIDI: Control Surfaces table (each row a control surface with its input and output), Takeover Mode, then the MIDI ports table (columns per the manual: Track, Sync, Remote, MPE) | | obs, man 2.3.5, 41.3 |
| L-SET-015 | File & Folder | Create Analysis Files; Sample Editor (Browse); Temporary Folder (Browse, path); Sound Content Analysis; Max Application (use bundled, Browse, path); Decoding Cache: Minimum Free Space, Limit Cache Size and Maximum Cache Size, Cleanup; cache folder (Browse, path) | | obs, man 2.3.6 |
| L-SET-016 | Library | Collect Files on Export; Show Downloadable Packs; Show Cloud; Show Push; Show Splice; Locations: pack installation folder and User Library (each Browse, path); Splice download folder | | obs, man 2.3.7 |
| L-SET-017 | Plug-Ins | Plug-In Sources: Rescan; Use Audio Units v2; Use Audio Units v3; VST2 system folders and custom folder (Browse, path); VST3 system folders and custom folder (Browse, path). Plug-In Windows: Multiple Plug-In Windows, Auto-Hide Plug-In Windows, Auto-Open Plug-In Windows. Rescan is a button; Audio Units v2 and v3 and the VST2 and VST3 system and custom folders default to Off, and the three Plug-In Windows switches to On (observed in Live 12, 12 October 2026) | | obs, man 2.3.8 |
| L-SET-018 | Record, Warp & Launch | Headings and rows as Live 12.4.6 shows them (observed 11 October 2026). Record: File Type (WAV), Bit Depth (24), Count-In (None), Exclusive (Arm and Solo toggles side by side, both on), Clip Update Rate (1/16; choices 1 Bar, 1/2, 1/4, 1/8, 1/16, 1/32), Record Session automation in (a two-way switch: Armed Tracks, the default, drawn orange, or All Tracks, drawn blue), Start Transport With Record (On). Warp/Fades: Loop/Warp Short Samples (Auto), Auto-Warp Long Samples (On), Default Warp Mode (Beats), Create Fades on Clip Edges (an On/Off toggle, On). Launch: Default Launch Mode (Trigger), Default Launch Quantization (Global), Select on Launch (On), Select Next Scene on Launch (On), Start Recording on Scene Launch (Off). Tap Tempo: Start Transport With Tap Tempo (On) | | obs (12.4.6), man 2.3.9; values obs (Live 12.4.6, 11 October 2026 and Live 12, 15 October 2026) |
| L-SET-019 | Licenses & Updates | Licenses (authorisation status and buttons); Maintenance: Get Automatic Updates, Send Usage Data | | obs, man 2.3.10 |
| L-SET-020 | Agents | The agent server's switch, status and address, the server's token, and the allowed agents, each with its autonomous switch and its own token. Each value (the address and every token) is a read-only field, selectable and copyable, with a Copy button beside it that puts the whole value on the clipboard. Below, the built-in assistant's section (WP-2.23, part of D24's assistant): the provider chooser, the base URL (for OpenAI-compatible local servers) and model fields, the API key entry (masked, stored to the OS keychain alone, with its Set and Forget buttons), and the activity log button that opens the list of every command the assistant ran, with its time and result. | | Beatlayer addition, decided 14 October 2026 (WP-1.15); the assistant's section added for WP-2.23 |

Beatlayer keeps the page order and grouping. Pages or rows tied to Ableton services (licences, Splice, Cloud, Push, Max) become Beatlayer's own settings or are left out; each such difference needs a decisions-log entry.

## Other windows and panels (L-OTH)

| ID | Element | Placement | Shown or hidden by | Source |
|---|---|---|---|---|
| L-OTH-001 | Undo History | A panel in the right-hand area, where the Learn View sits (L-WIN-008); not a separate window. Lists the set's actions newest first, each named for its action (for example Change "Track Volume", Insert Clip, with New Live Set at the bottom); undone steps stay listed, greyed, above the current one until a new edit replaces them. Click a step to go back to it | M-VIEW-008, S-VIEW-021 | obs, man 5.4.2 |
| L-OTH-002 | File Manager | Opens in the detail area's place, with Manage Set, Manage Project, and missing-files sections | M-FILE-007, M-VIEW-007, a missing-files message in the Status Bar | man 5.4.6, 5.6 |
| L-OTH-003 | Export Audio/Video dialog | Modal dialog. The manual's sections (selection, rendering, encoding, video) are summarised in [the export spec](../specs/export.md). Exports run through `export.audio` and `bl-cli export` | M-FILE-015, S-FILE-007 | man 5.1.3 |
| L-OTH-004 | Video window | Floating window above the main window; position and size kept between sets | M-VIEW-029, S-VIEW-009 | man 28.2.2 |
| L-OTH-005 | Scene View | See L-SES-011 | | man 7.2.2 |
| L-OTH-006 | Plug-in windows | One floating window per plug-in editor (several only with the Multiple Plug-In Windows setting). Observed in Live 12.4.6 on 12 October 2026: a small-title-bar utility window above the main window, titled `<device>/<track>` (`AUDelay/4-Audio`); with Auto-Hide Plug-In Windows on (the default) it hides while another track is selected. Beatlayer's floating native devices (IMP-14, WP-2.5b) follow it: a 16-point title bar on macOS, a tool window on Windows, a utility window on X11 | Device's window button, M-VIEW-028, S-DEV-009 | man 24.1.1, obs |
| L-OTH-007 | MIDI and Key Map Mode overlays, and the Mapping Browser | Mappable controls are tinted; the mappings list replaces the browser while a map mode is on | L-CTRL-021, L-CTRL-022, S-MAP-001, S-MAP-002 | man 34 |
| L-OTH-008 | MPE/Multi-channel Settings dialog | Dialog from a plug-in's context menu | C-DEVICE-015 | man 12.6 |

# Keyboard shortcuts

<!-- Generated from shortcuts.toml by `cargo xtask ui-parity`. Don't edit this file by hand: edit shortcuts.toml and regenerate. `cargo xtask check` fails if the two differ. -->

Live 12.4.3's keyboard shortcuts, the default keymap Beatlayer binds (D16). Source: Ableton Live 12 reference manual, chapters 41 and 42, read 10 October 2026, and observation of Live 12.4.3 where the Source column says so. 377 shortcuts in 24 contexts; 19 need confirming (see the notes at the end). How to read and change this table: [README](README.md).

## By milestone

| Milestone | Shortcuts |
|---|---|
| M1 | 137 |
| M2 | 87 |
| M3 | 39 |
| M4 | 21 |
| M5 | 10 |
| M6 | 71 |
| M7 | 2 |
| M9 | 8 |
| M10 | 2 |

## Contexts

A shortcut applies in its context. The same keys may mean different things in different contexts; within one context they must not, unless the notes explain it.

| Context | Applies when | Shortcuts |
|---|---|---|
| [`global`](#global) | Anywhere in a main window, unless a narrower context binds the same keys. | 105 |
| [`dialog`](#dialog) | A secondary window or dialog has focus. | 1 |
| [`tab-focus`](#tab-focus) | Use Tab Key to Move Focus is on (Navigate menu or Display & Input settings). | 4 |
| [`text-field`](#text-field) | A name or text is being edited. | 2 |
| [`value-field`](#value-field) | A value control (knob, slider, number box) has focus. | 9 |
| [`menu`](#menu) | A menu is open. | 3 |
| [`session`](#session) | The Session View has focus. | 28 |
| [`arrangement`](#arrangement) | The Arrangement View has focus. | 48 |
| [`track-headers`](#track-headers) | A track title bar is selected, in either view. | 13 |
| [`take-lanes`](#take-lanes) | A take lane is selected in the Arrangement. | 6 |
| [`loop-brace`](#loop-brace) | A loop brace or a clip start or end marker is selected, or a multi-clip loop bar. | 11 |
| [`envelope`](#envelope) | An automation lane or the Clip View's Envelope Editor has focus. | 16 |
| [`clip-view`](#clip-view) | The Clip View has focus, whichever editor it shows. | 11 |
| [`sample-editor`](#sample-editor) | The Clip View shows an audio clip's Sample Editor. | 16 |
| [`note-editor`](#note-editor) | The Clip View shows a MIDI clip's MIDI Note Editor. | 41 |
| [`note-selection`](#note-selection) | Notes are selected in the MIDI Note Editor. | 16 |
| [`device-view`](#device-view) | The Device View has focus. | 12 |
| [`rack-chains`](#rack-chains) | A rack's chain list has focus. | 1 |
| [`sample-swap`](#sample-swap) | A Drum Rack, a pad, Simpler or Drum Sampler is selected. | 5 |
| [`browser`](#browser) | The browser's content pane has focus. | 15 |
| [`browser-sidebar`](#browser-sidebar) | The browser's sidebar has focus. | 2 |
| [`browser-search`](#browser-search) | The browser's search field has focus. | 1 |
| [`computer-midi-keyboard`](#computer-midi-keyboard) | The computer MIDI keyboard is on. | 4 |
| [`settings`](#settings) | The Settings window has focus. | 7 |

## global

Anywhere in a main window, unless a narrower context binds the same keys.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-VIEW-001 | Toggle full-screen mode | `Ctrl+Cmd+F` | `F11` | key | press | M1 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-002 | Show or hide the second window | `Cmd+Shift+W` | `Ctrl+Shift+W` | key | press | M6 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: pressed, and View menu |
| S-VIEW-003 | Switch between Session and Arrangement (when Tab doesn't move focus) | `Tab` | `Tab` | key | latch | M3 | manual 42.1, 42.25; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-004 | Switch the detail area between Device View and Clip View | `Shift+Tab` or `F12` | `Shift+Tab` or `F12` | key | press | M2 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu (Shift+Tab) |
| S-VIEW-005 | Show Device View and Clip View together (stack them) | Option+click a view toggle | Alt+click a view toggle | mouse | press | M6 | manual 42.1 |
| S-VIEW-006 | Toggle hot-swap mode for the selected device or item | `Q` | `Q` | key | press | M5 | manual 42.1 |
| S-VIEW-007 | Switch between a Drum Rack and its last selected pad | `D` | `D` | key | press | M5 | manual 42.1 |
| S-VIEW-008 | Show or hide the Info View | `Shift+Slash` | `Shift+Slash` | key | press | M2 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-009 | Show or hide the video window (unconfirmed) | `Cmd+Option+Shift+V` | `Ctrl+Alt+Shift+V` | key | press | M9 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-010 | Show or hide the browser | `Cmd+Option+B` or `Cmd+Option+5` | `Ctrl+Alt+B` or `Ctrl+Alt+5` | key | press | M1 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu (Cmd+Option+5) |
| S-VIEW-011 | Show or hide the Arrangement Overview | `Cmd+Option+O` | `Ctrl+Alt+O` | key | press | M1 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-012 | Show or hide the mixer's In/Out section | `Cmd+Option+I` | `Ctrl+Alt+I` | key | press | M1 | manual 42.1; observed in 12.4.6 (macOS), 11 October 2026: View menu, Mixer Controls |
| S-VIEW-013 | Show or hide the mixer's Sends section | `Cmd+Option+S` | `Ctrl+Alt+S` | key | press | M2 | manual 42.1; observed in 12.4.6 (macOS), 11 October 2026: View menu, Mixer Controls |
| S-VIEW-014 | Show or hide the mixer (in the Arrangement) | `Cmd+Option+M` | `Ctrl+Alt+M` | key | press | M1 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: pressed, and View menu |
| S-VIEW-015 | Show or hide the Clip View | `Cmd+Option+3` | `Ctrl+Alt+3` | key | press | M2 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-016 | Show or hide the Device View | `Cmd+Option+4` | `Ctrl+Alt+4` | key | press | M2 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-017 | Show or hide the Groove Pool | `Cmd+Option+6` | `Ctrl+Alt+6` | key | press | M6 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: pressed, and View menu |
| S-VIEW-018 | Show or hide the Learn View | `Cmd+Option+7` | `Ctrl+Alt+7` | key | press | M10 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: pressed, and View menu |
| S-VIEW-019 | Open the Settings window | `Cmd+Comma` | `Ctrl+Comma` | key | press | M1 | manual 42.1; observed in 12.4.3 (macOS), 10 October 2026: application menu |
| S-FOCUS-001 | Move focus to the Control Bar | `Option+0` | `Alt+0` | key | press | M1 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-002 | Move focus to the Session View | `Option+1` | `Alt+1` | key | press | M3 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-003 | Move focus to the Arrangement View | `Option+2` | `Alt+2` | key | press | M1 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-004 | Move focus to the Clip View's editor | `Option+3` | `Alt+3` | key | press | M2 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-005 | Move focus to the Device View | `Option+4` | `Alt+4` | key | press | M2 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-006 | Move focus to the browser | `Option+5` | `Alt+5` | key | press | M1 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-007 | Move focus to the Groove Pool | `Option+6` | `Alt+6` | key | press | M6 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-008 | Move focus to the Learn View | `Option+7` | `Alt+7` | key | press | M10 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-009 | Move focus to the selected clip panel | `Option+8` | `Alt+8` | key | press | M2 | manual 42.2 |
| S-FOCUS-010 | Move focus to the clip panels | `Option+Shift+P` | `Alt+Shift+P` | key | press | M2 | manual 42.2; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FILE-001 | Create a new set | `Cmd+N` | `Ctrl+N` | key | press | M1 | manual 42.3; observed in 12.4.3 (macOS), 10 October 2026: File menu |
| S-FILE-002 | Open a set | `Cmd+O` | `Ctrl+O` | key | press | M1 | manual 42.3; observed in 12.4.3 (macOS), 10 October 2026: File menu |
| S-FILE-003 | Save the set | `Cmd+S` | `Ctrl+S` | key | press | M1 | manual 42.3; observed in 12.4.3 (macOS), 10 October 2026: File menu |
| S-FILE-004 | Save the set under a new name | `Cmd+Shift+S` | `Ctrl+Shift+S` | key | press | M1 | manual 42.3; observed in 12.4.3 (macOS), 10 October 2026: File menu |
| S-FILE-005 | Quit the application | `Cmd+Q` | `Ctrl+Q` | key | press | M1 | manual 42.3; observed in 12.4.3 (macOS), 10 October 2026: application menu |
| S-FILE-006 | Hide the application | `Cmd+H` | none | key | press | M1 | manual 42.3; observed in 12.4.3 (macOS), 10 October 2026: application menu |
| S-FILE-007 | Open the audio and video export dialog | `Cmd+Shift+R` | `Ctrl+Shift+R` | key | press | M1 | manual 42.3; observed in 12.4.3 (macOS), 10 October 2026: File menu |
| S-FILE-008 | Export the selected MIDI clip as a MIDI file (unconfirmed) | `Cmd+Shift+E` | `Ctrl+Shift+E` | key | press | M2 | manual 42.3 |
| S-DEV-005 | List the devices on a track | Right-click the Device View selector | Right-click the Device View selector | mouse | press | M2 | manual 42.4 |
| S-DEV-009 | Show or hide all open plug-in windows | `Cmd+Option+P` | `Ctrl+Alt+P` | key | press | M2 | manual 42.4; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-EDIT-001 | Cut | `Cmd+X` | `Ctrl+X` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-EDIT-002 | Copy | `Cmd+C` | `Ctrl+C` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-EDIT-003 | Paste | `Cmd+V` | `Ctrl+V` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-EDIT-004 | Duplicate | `Cmd+D` | `Ctrl+D` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-EDIT-005 | Delete | `Delete` | `Delete` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-EDIT-006 | Undo | `Cmd+Z` | `Ctrl+Z` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-EDIT-007 | Redo | `Cmd+Shift+Z` | `Ctrl+Y` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-EDIT-008 | Rename the selection | `Cmd+R` | `Ctrl+R` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-EDIT-009 | Select everything in the focused view | `Cmd+A` | `Ctrl+A` | key | press | M1 | manual 42.5; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-EDIT-010 | Add or remove one item from the selection | Cmd+click | Ctrl+click | mouse | press | M1 | manual 42.5 |
| S-EDIT-011 | Select a range from the first to the last clicked item | Shift+click | Shift+click | mouse | press | M1 | manual 42.5 |
| S-EDIT-013 | Ignore grid snapping while dragging | Cmd (hold while dragging) | Alt (hold while dragging) | modifier | held | M1 | manual 42.5 |
| S-ZOOM-001 | Zoom the interface in | `Cmd+Plus` | `Ctrl+Plus` | key | press | M1 | manual 42.9 |
| S-ZOOM-002 | Zoom the interface out | `Cmd+Minus` | `Ctrl+Minus` | key | press | M1 | manual 42.9 |
| S-ZOOM-005 | Scroll the display to follow playback (unconfirmed) | `Option+Shift+F` | `Alt+Shift+F` | key | press | M1 | manual 42.9; observed in 12.4.3 (macOS), 15 October 2026: Control Bar, Follow button |
| S-ZOOM-006 | Scroll left or right | Shift+scroll | Shift+scroll | mouse | press | M1 | manual 42.9 |
| S-ZOOM-007 | Pan the display around the selection | Cmd+Option+drag | Ctrl+Alt+drag | mouse | held | M1 | manual 42.9 |
| S-ZOOM-008 | Add items to the selection | Shift+click or Shift+drag | Shift+click or Shift+drag | mouse | press | M1 | manual 42.9 |
| S-ZOOM-009 | Add adjacent clips, tracks or scenes to the selection | Shift+click | Shift+click | mouse | press | M1 | manual 42.9 |
| S-ZOOM-010 | Add non-adjacent clips, tracks or scenes to the selection | Cmd+click | Ctrl+click | mouse | press | M1 | manual 42.9 |
| S-GRID-001 | Toggle Draw Mode | `B` | `B` | key | latch | M2 | manual 42.13, 42.25; observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-GRID-002 | Narrow the grid | `Cmd+1` | `Ctrl+1` | key | press | M1 | manual 42.13; observed in 12.4.3 (macOS), 10 October 2026: Arrangement clip and time-selection context menus |
| S-GRID-003 | Widen the grid | `Cmd+2` | `Ctrl+2` | key | press | M1 | manual 42.13; observed in 12.4.3 (macOS), 10 October 2026: Arrangement clip and time-selection context menus |
| S-GRID-004 | Toggle a triplet grid | `Cmd+3` | `Ctrl+3` | key | press | M1 | manual 42.13; observed in 12.4.3 (macOS), 10 October 2026: Arrangement clip and time-selection context menus |
| S-GRID-005 | Toggle snapping to the grid | `Cmd+4` | `Ctrl+4` | key | press | M1 | manual 42.13; observed in 12.4.3 (macOS), 10 October 2026: Arrangement clip and time-selection context menus |
| S-GRID-006 | Switch between a fixed and a zoom-adaptive grid | `Cmd+5` | `Ctrl+5` | key | press | M1 | manual 42.13; observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-GRID-007 | Bypass snapping while dragging | Cmd (hold while dragging) | Alt (hold while dragging) | modifier | held | M1 | manual 42.13 |
| S-GQ-001 | Set global launch quantisation to a sixteenth note | `Cmd+6` | `Ctrl+6` | key | press | M3 | manual 42.14; observed in 12.4.3 (macOS), 15 October 2026: Control Bar, Quantization menu |
| S-GQ-002 | Set global launch quantisation to an eighth note | `Cmd+7` | `Ctrl+7` | key | press | M3 | manual 42.14; observed in 12.4.3 (macOS), 15 October 2026: Control Bar, Quantization menu |
| S-GQ-003 | Set global launch quantisation to a quarter note | `Cmd+8` | `Ctrl+8` | key | press | M3 | manual 42.14; observed in 12.4.3 (macOS), 15 October 2026: Control Bar, Quantization menu |
| S-GQ-004 | Set global launch quantisation to one bar | `Cmd+9` | `Ctrl+9` | key | press | M3 | manual 42.14; observed in 12.4.3 (macOS), 15 October 2026: Control Bar, Quantization menu |
| S-GQ-005 | Turn global launch quantisation off | `Cmd+0` | `Ctrl+0` | key | press | M3 | manual 42.14; observed in 12.4.3 (macOS), 15 October 2026: Control Bar, Quantization menu |
| S-BOUNCE-001 | Bounce the selection to a new audio track | `Cmd+B` | `Ctrl+B` | key | press | M4 | manual 42.18; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-TRACK-001 | Insert an audio track | `Cmd+T` | `Ctrl+T` | key | press | M1 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Create menu and the track header's context menu |
| S-TRACK-002 | Insert a MIDI track | `Cmd+Shift+T` | `Ctrl+Shift+T` | key | press | M2 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Create menu and the track header's context menu |
| S-TRACK-003 | Insert a return track | `Cmd+Option+T` | `Ctrl+Alt+T` | key | press | M2 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Create menu and the track header's context menu |
| S-TRACK-011 | Show or hide the return tracks | `Cmd+Option+R` | `Ctrl+Alt+R` | key | press | M2 | manual 42.19; observed in 12.4.6 (macOS), 11 October 2026: View menu, Mixer Controls |
| S-TRANS-001 | Play from the start marker, or stop | `Space` | `Space` | key | press | M1 | manual 42.20; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-TRANS-002 | Continue playing from where playback stopped | `Shift+Space` | `Shift+Space` | key | press | M1 | manual 42.20; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-TRANS-003 | Play and stop at the end of the selection | `Option+Space` | `Ctrl+Space` | key | press | M1 | manual 42.20 |
| S-TRANS-004 | Play the Arrangement selection | `Space` | `Space` | key | press | M1 | manual 42.20 |
| S-TRANS-005 | Move the insert marker to the start | `Home` or `Fn+Left` | `Home` | key | press | M1 | manual 42.20 |
| S-TRANS-006 | Record | `F9` | `F9` | key | press | M2 | manual 42.20; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-TRANS-007 | Arm recording in the Arrangement | `Shift+F9` | `Shift+F9` | key | press | M2 | manual 42.20; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-TRANS-008 | Record into the Session View | `Cmd+Shift+F9` | `Ctrl+Shift+F9` | key | press | M3 | manual 42.20; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-TRANS-009 | Return every track to the Arrangement (Back to Arrangement) | `F10` | `F10` | key | press | M3 | manual 42.20; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-TRANS-010 | Turn track 1 to 8 on or off | `F1..F8` | `F1..F8` | key | latch | M1 | manual 42.20, 42.25 |
| S-TRANS-011 | Turn the metronome on or off | `O` | `O` | key | press | M2 | manual 42.20; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-ENGINE-001 | Turn the audio engine on or off | `Cmd+Option+Shift+E` | `Ctrl+Alt+Shift+E` | key | press | M1 | manual 42.21; observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-BRW-002 | Show or hide the browser | `Cmd+Option+B` | `Ctrl+Alt+B` | key | press | M1 | manual 42.22 |
| S-BRW-006 | Search the browser | `Cmd+F` | `Ctrl+F` | key | press | M1 | manual 42.22; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-MAP-001 | Turn MIDI Map Mode on or off | `Cmd+M` | `Ctrl+M` | key | press | M7 | manual 42.24; observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-MAP-002 | Turn Key Map Mode on or off | `Cmd+K` | `Ctrl+K` | key | press | M7 | manual 42.24; observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-MAP-003 | Turn the computer MIDI keyboard on or off | `M` | `M` | key | press | M2 | manual 42.24; observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-MENU-001 | Open a menu by the first letter of its title (unconfirmed) | none | `Alt+F` or `Alt+E` or `Alt+C` or `Alt+P` or `Alt+V` or `Alt+N` or `Alt+O` or `Alt+H` | key | press | M1 | manual 42.28, 41.1.3, 41.4.1 |
| S-MENU-005 | Search the menus for a command | `Cmd+Shift+Slash` | none | key | press | M1 | manual 42.28 |
| S-CTX-001 | Open the context menu of the item under the pointer | Right-click or Ctrl+click | Right-click | mouse | press | M1 | manual 42.29 |
| S-CTX-002 | Open the context menu of the focused item from the keyboard | `Ctrl+Option+Shift+M` | `Menu` or `Shift+F10` | key | press | M1 | manual 42.29 |
| S-VIEW-021 | Open the Undo History | `Cmd+Option+Z` | `Ctrl+Alt+Z` | key | press | M6 | manual 5.4.2; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-FILE-009 | Close the frontmost window | `Cmd+W` | `Ctrl+W` | key | press | M1 | observed in 12.4.3 (macOS), 10 October 2026: File menu |
| S-FILE-010 | Hide the other applications | `Cmd+Option+H` | none | key | press | M1 | observed in 12.4.3 (macOS), 10 October 2026: application menu |
| S-VIEW-022 | Hide or show the detail area (the Clip View or Device View) | `Cmd+Option+L` | `Ctrl+Alt+L` | key | press | M2 | observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-VIEW-023 | Show the Max for Live window | `Cmd+Option+Shift+M` | `Ctrl+Alt+Shift+M` | key | press | M9 | observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-MENU-006 | Open the system's emoji and symbols picker | `Ctrl+Cmd+Space` | none | key | press | M1 | observed in 12.4.3 (macOS), 10 October 2026: Edit menu (Emoji & Symbols) |
| S-MENU-007 | Open the command search | `Cmd+Shift+P` | `Ctrl+Shift+P` | key | press | M6 | Beatlayer (IMP-16, WP-6.42); not in Live |
| S-FOCUS-015 | Open the track search | `Cmd+Shift+O` | `Ctrl+Shift+O` | key | press | M6 | Beatlayer (IMP-18, WP-6.42); not in Live |

## dialog

A secondary window or dialog has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-VIEW-020 | Close the frontmost window or dialog | `Esc` | `Esc` | key | press | M1 | manual 42.1 |

## tab-focus

Use Tab Key to Move Focus is on (Navigate menu or Display & Input settings).

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-FOCUS-011 | Move to the next focusable control | `Tab` | `Tab` | key | press | M1 | manual 42.2, 42.26.1 |
| S-FOCUS-012 | Move to the previous focusable control | `Shift+Tab` | `Shift+Tab` | key | press | M1 | manual 42.2, 42.26.1 |
| S-FOCUS-013 | Move to the same control on the next track (next neighbour) | `Option+Tab` | `Ctrl+Tab` | key | press | M1 | manual 42.2, 42.26.1; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-FOCUS-014 | Move to the same control on the previous track (previous neighbour) | `Option+Shift+Tab` | `Ctrl+Shift+Tab` | key | press | M1 | manual 42.2, 42.26.1; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |

## text-field

A name or text is being edited.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-EDIT-012 | While renaming, move to the next track or scene | `Tab` | `Tab` | key | press | M1 | manual 42.5 |
| S-TRACK-005 | While renaming, move to the next track | `Tab` | `Tab` | key | press | M1 | manual 42.19 |

## value-field

A value control (knob, slider, number box) has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-DEV-006 | Reset a parameter to its default value | `Delete` | `Delete` | key | press | M2 | manual 42.4; observed in 12.4.6 (macOS), 12 October 2026: pressed on a device knob |
| S-VAL-001 | Decrease or increase the focused value | `Down` or `Up` | `Down` or `Up` | key | press | M1 | manual 42.6; observed in 12.4.6 (macOS), 11 October 2026: pressed on a track's volume field |
| S-VAL-002 | Decrease or increase by octaves, or in fine steps | `Shift+Down` or `Shift+Up` | `Shift+Down` or `Shift+Up` | key | press | M1 | manual 42.6; observed in 12.4.6 (macOS), 11 October 2026: pressed on a track's volume field |
| S-VAL-003 | Adjust in finer steps while dragging | Shift (hold while dragging) | Shift (hold while dragging) | modifier | held | M1 | manual 42.6; observed in 12.4.6 (macOS), 11 October 2026: held while dragging a pan knob |
| S-VAL-004 | Return the value to its default | `Delete` | `Delete` | key | press | M1 | manual 42.6; observed in 12.4.6 (macOS), 12 October 2026: pressed on a device knob |
| S-VAL-005 | Type a value | `0..9` | `0..9` | key | press | M1 | manual 42.6 |
| S-VAL-006 | Move to the next field of a bar-beat-sixteenth value | `Period` or `Comma` | `Period` or `Comma` | key | press | M1 | manual 42.6 |
| S-VAL-007 | Cancel value entry | `Esc` | `Esc` | key | press | M1 | manual 42.6 |
| S-VAL-008 | Confirm value entry | `Enter` | `Enter` | key | press | M1 | manual 42.6 |

## menu

A menu is open.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-MENU-002 | Move up or down a menu | `Up` or `Down` | `Up` or `Down` | key | press | M1 | manual 42.28 |
| S-MENU-003 | Open the neighbouring menu | `Left` or `Right` | `Left` or `Right` | key | press | M1 | manual 42.28 |
| S-MENU-004 | Choose the highlighted menu item | `Enter` | `Enter` | key | press | M1 | manual 42.28 |

## session

The Session View has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-EDIT-014 | Apply an editing command to clips and slots across all tracks | Shift (with the command) | Shift (with the command) | modifier | held | M3 | manual 42.5 |
| S-SES-001 | Launch the selected clip, slot or scene | `Enter` | `Enter` | key | press | M3 | manual 42.15 |
| S-SES-002 | Select the neighbouring clip or slot | `Up` or `Down` or `Left` or `Right` | `Up` or `Down` or `Left` or `Right` | key | press | M3 | manual 42.15 |
| S-SES-003 | Select all clips and slots | `Cmd+A` | `Ctrl+A` | key | press | M3 | manual 42.15 |
| S-SES-004 | Copy clips by dragging | Option+drag | Ctrl+drag | mouse | press | M3 | manual 42.15 |
| S-SES-005 | Add or remove a clip stop button | `Cmd+E` | `Ctrl+E` | key | press | M3 | manual 42.15; observed in 12.4.3 (macOS), 10 October 2026: Edit menu, and the empty slot's and Session clip's context menus |
| S-SES-006 | Stop the clips in the tracks that have a selected slot | `Cmd+Enter` | `Ctrl+Enter` | key | press | M3 | manual 42.15 |
| S-SES-007 | Insert an empty MIDI clip | `Cmd+Shift+M` | `Ctrl+Shift+M` | key | press | M3 | manual 42.15; observed in 12.4.3 (macOS), 10 October 2026: Create menu and the empty slot's context menu |
| S-SES-008 | Insert a scene | `Cmd+I` | `Ctrl+I` | key | press | M3 | manual 42.15; observed in 12.4.3 (macOS), 10 October 2026: Create menu and the scene context menu |
| S-SES-009 | Capture the playing clips into a new scene | `Cmd+Shift+I` | `Ctrl+Shift+I` | key | press | M3 | manual 42.15; observed in 12.4.3 (macOS), 10 October 2026: Create menu and the scene context menu |
| S-SES-010 | Move to the scene above or below | `Up` or `Down` | `Up` or `Down` | key | press | M3 | manual 42.15 |
| S-SES-011 | Move eight scenes up or down | `PageUp` or `PageDown` | `PageUp` or `PageDown` | key | press | M3 | manual 42.15 |
| S-SES-012 | Record into the Session View | `Cmd+Shift+F9` | `Ctrl+Shift+F9` | key | press | M3 | manual 42.15; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-SES-013 | Turn follow actions on or off for the selected clips | `Shift+Enter` | `Shift+Enter` | key | press | M3 | manual 42.15 |
| S-SES-014 | Create a follow action chain from the selected clips | `Cmd+Shift+Enter` | `Ctrl+Shift+Enter` | key | press | M3 | manual 42.15; observed in 12.4.3 (macOS), 10 October 2026: Session clip's context menu |
| S-SES-015 | Move the selected track left or right | `Cmd+Left` or `Cmd+Right` | `Ctrl+Left` or `Ctrl+Right` | key | press | M3 | manual 42.15 |
| S-SES-016 | Move non-adjacent scenes without collapsing them | `Cmd+Up` or `Cmd+Down` | `Ctrl+Up` or `Ctrl+Down` | key | press | M3 | manual 42.15 |
| S-SES-017 | Drop clips from the browser as a scene | Cmd (hold while dropping) | Ctrl (hold while dropping) | modifier | held | M3 | manual 42.15 |
| S-SES-018 | Deactivate the selected clip | `0` | `0` | key | press | M3 | manual 42.15; observed in 12.4.3 (macOS), 10 October 2026: Session clip's context menu |
| S-SES-019 | Jump to the highlighted track's title bar | `Esc` | `Esc` | key | press | M3 | manual 42.15 |
| S-SES-020 | Jump to the first track of the current scene | `Home` or `Fn+Left` | `Home` | key | press | M3 | manual 42.15 |
| S-SES-021 | Jump to the last track of the current scene | `End` or `Fn+Right` | `End` | key | press | M3 | manual 42.15 |
| S-SES-022 | From the last track, jump to the current scene | `End` or `Fn+Right` | `End` | key | press | M3 | manual 42.15 |
| S-SES-024 | Make launch quantisation smaller | `Cmd+1` | `Ctrl+1` | key | press | M3 | observed in 12.4.3 (macOS), 10 October 2026: Options menu (Smaller Launch Quantization) |
| S-SES-025 | Make launch quantisation larger | `Cmd+2` | `Ctrl+2` | key | press | M3 | observed in 12.4.3 (macOS), 10 October 2026: Options menu (Larger Launch Quantization) |
| S-SES-026 | Toggle triplet launch quantisation | `Cmd+3` | `Ctrl+3` | key | press | M3 | observed in 12.4.3 (macOS), 10 October 2026: Options menu (Triplet Launch Quantization) |
| S-SES-027 | Turn launch quantisation on or off | `Cmd+4` | `Ctrl+4` | key | press | M3 | observed in 12.4.3 (macOS), 10 October 2026: Options menu (Launch Quantization Enabled) |
| S-SES-028 | Crop the selected clip | `Cmd+Shift+J` | `Ctrl+Shift+J` | key | press | M4 | observed in 12.4.3 (macOS), 10 October 2026: empty slot's and Session clip's context menus (Crop Clip) |

## arrangement

The Arrangement View has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-EDIT-015 | Apply an editing command to time across all tracks | Shift (with the command) | Shift (with the command) | modifier | held | M6 | manual 42.5 |
| S-ENV-001 | Show or hide automation lanes (Automation Mode) | `A` | `A` | key | latch | M2 | manual 42.7, 42.25; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-ENV-004 | Show fade handles while held, over an automation lane | `F` | `F` | key | held | M1 | manual 42.7 |
| S-ZOOM-003 | Zoom the time ruler in | `Plus` | `Plus` | key | press | M1 | manual 42.9 |
| S-ZOOM-004 | Zoom the time ruler out | `Minus` | `Minus` | key | press | M1 | manual 42.9 |
| S-ARR-001 | Split the clip at the selection or insert marker | `Cmd+E` | `Ctrl+E` | key | press | M1 | manual 42.16 |
| S-ARR-002 | Consolidate the selection into one clip | `Cmd+J` | `Ctrl+J` | key | press | M4 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-ARR-003 | Crop the selected clips | `Cmd+Shift+J` | `Ctrl+Shift+J` | key | press | M4 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Arrangement clip's context menu |
| S-ARR-004 | Resize a clip from its edge: Enter, then arrows, then Enter (Esc cancels) | Enter then Left or Right | Enter then Left or Right | sequence | press | M1 | manual 42.16 |
| S-ARR-005 | Slide a clip's contents inside the clip (unconfirmed) | Shift+Option+drag | Shift+Alt+drag | mouse | held | M2 | manual 42.16 |
| S-ARR-006 | Stretch a warped clip | Shift+drag the clip title bar | Shift+drag the clip title bar | mouse | held | M4 | manual 42.16 |
| S-ARR-007 | Select time inside a clip | Shift+Option+drag the clip title bar | Shift+Alt+drag the clip title bar | mouse | held | M1 | manual 42.16 |
| S-ARR-008 | Create a fade or crossfade from the selection | `Cmd+Option+F` | `Ctrl+Alt+F` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Create menu |
| S-ARR-009 | Delete the fades in the selected clips | `Cmd+Option+Delete` | `Ctrl+Alt+Backspace` | key | press | M1 | manual 42.16 |
| S-ARR-010 | Show fade handles while held | `F` | `F` | key | held | M1 | manual 42.16 |
| S-ARR-011 | Loop the selection, or toggle the Arrangement loop | `Cmd+L` | `Ctrl+L` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu, and the Arrangement clip and time-selection context menus |
| S-ARR-013 | Select the loop brace's contents | `Cmd+Shift+L` | `Ctrl+Shift+L` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-ARR-014 | Insert silence at the insert marker | `Cmd+I` | `Ctrl+I` | key | press | M6 | manual 42.16 |
| S-ARR-015 | Cut time across all tracks | `Cmd+Shift+X` | `Ctrl+Shift+X` | key | press | M6 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-ARR-016 | Copy time across all tracks | `Cmd+Shift+C` | `Ctrl+Shift+C` | key | press | M6 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-ARR-017 | Paste time across all tracks | `Cmd+Shift+V` | `Ctrl+Shift+V` | key | press | M6 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-ARR-018 | Duplicate time across all tracks | `Cmd+Shift+D` | `Ctrl+Shift+D` | key | press | M6 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-ARR-019 | Delete time across all tracks | `Cmd+Shift+Delete` | `Ctrl+Shift+Delete` | key | press | M6 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-ARR-021 | Unfold every track | `Option+U` | `Alt+U` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-ARR-022 | Make the selected tracks or clips taller or shorter | `Option+Plus` or `Option+Minus` | `Alt+Plus` or `Alt+Minus` | key | press | M1 | manual 42.16, 21.2 |
| S-ARR-023 | Scroll the display to follow playback (unconfirmed) | `Option+Shift+F` | `Ctrl+Shift+F` | key | press | M1 | manual 42.16 |
| S-ARR-024 | Pan the display around the selection | Cmd+Option+drag | Ctrl+Alt+drag | mouse | held | M1 | manual 42.16 |
| S-ARR-025 | Fit all tracks to the Arrangement's height | `H` | `H` | key | press | M1 | manual 42.16 |
| S-ARR-026 | Fit the Arrangement to the window's width | `W` | `W` | key | press | M1 | manual 42.16 |
| S-ARR-027 | Deactivate the selection | `0` | `0` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Arrangement clip's context menu |
| S-ARR-028 | Nudge the selection left or right | `Left` or `Right` | `Left` or `Right` | key | press | M1 | manual 42.16 |
| S-ARR-029 | Reverse the selected clips | `R` | `R` | key | press | M4 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Arrangement clip's context menu |
| S-ARR-030 | Zoom to the time selection | `Z` | `Z` | key | latch | M1 | manual 42.16, 42.25; observed in 12.4.3 (macOS), 10 October 2026: View menu, and the Arrangement clip and time-selection context menus |
| S-ARR-031 | Zoom back from the time selection | `X` | `X` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: View menu, and the Arrangement clip and time-selection context menus |
| S-ARR-032 | Play from the insert marker in the selected clip | `Option+Space` | `Ctrl+Space` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-ARR-033 | Move the insert marker to the play position | `Cmd+Shift+Space` | `Ctrl+Shift+Space` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Playback menu |
| S-ARR-034 | Jump to the highlighted track's title bar | `Esc` | `Esc` | key | press | M1 | manual 42.16 |
| S-ARR-035 | Move focus to the mixer | `Option+Shift+M` | `Alt+Shift+M` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-COMP-001 | Show the take lanes | `Cmd+Option+U` | `Ctrl+Alt+U` | key | press | M6 | manual 42.17 |
| S-COMP-004 | Add a take lane | `Shift+Option+T` | `Shift+Alt+T` | key | press | M6 | manual 42.17; observed in 12.4.3 (macOS), 10 October 2026: Create menu |
| S-BOUNCE-002 | Paste bounced audio into the lane or slot | `Cmd+Option+V` | `Ctrl+Alt+V` | key | press | M4 | manual 42.18; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-A11Y-001 | Move the insert marker to the previous or next clip edge or locator | `Option+Left` or `Option+Right` | `Ctrl+Left` or `Ctrl+Right` | key | press | M1 | manual 41.4.1.3, 6.9 |
| S-A11Y-002 | Extend or shorten the selection | `Shift+Left` or `Shift+Right` or `Shift+Up` or `Shift+Down` | `Shift+Left` or `Shift+Right` or `Shift+Up` or `Shift+Down` | key | press | M1 | manual 41.4.1.3, 6.9 |
| S-A11Y-003 | Move between tracks | `Up` or `Down` | `Up` or `Down` | key | press | M1 | manual 41.4.1.3 |
| S-A11Y-026 | Cycle the track's automated parameters | `Option+Up` or `Option+Down` | `Alt+Up` or `Alt+Down` | key | press | M6 | manual 41.5 |
| S-A11Y-027 | Cycle all of the track's device parameters | `Shift+Option+Up` or `Shift+Option+Down` | `Shift+Alt+Up` or `Shift+Alt+Down` | key | press | M6 | manual 41.5 |
| S-ARR-036 | Slide a clip's contents without snapping to the grid | Cmd+Option+Shift+drag | Ctrl+Alt+Shift+drag | mouse | held | M2 | manual 6.7 |
| S-ARR-037 | Insert an empty MIDI clip in the time selection | `Cmd+Shift+M` | `Ctrl+Shift+M` | key | press | M2 | observed in 12.4.3 (macOS), 10 October 2026: Arrangement time selection's context menu |

## track-headers

A track title bar is selected, in either view.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-ARR-020 | Fold or unfold the selected tracks | `U` or `Left` or `Right` | `U` or `Left` or `Right` | key | press | M1 | manual 42.16; observed in 12.4.3 (macOS), 10 October 2026: View menu (U) |
| S-TRACK-004 | Rename the selected track | `Cmd+R` | `Ctrl+R` | key | press | M1 | manual 42.19 |
| S-TRACK-006 | Group the selected tracks | `Cmd+G` | `Ctrl+G` | key | press | M1 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and the track header's context menu |
| S-TRACK-007 | Ungroup the selected group track | `Cmd+Shift+G` | `Ctrl+Shift+G` | key | press | M1 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-TRACK-008 | Show the tracks inside a group | `Plus` | `Plus` | key | press | M1 | manual 42.19 |
| S-TRACK-009 | Hide the tracks inside a group | `Minus` | `Minus` | key | press | M1 | manual 42.19 |
| S-TRACK-010 | Collapse or expand a group track | `U` | `U` | key | press | M1 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-TRACK-012 | Move non-adjacent tracks without collapsing them | `Cmd+Up` or `Cmd+Down` or `Cmd+Left` or `Cmd+Right` | `Ctrl+Up` or `Ctrl+Down` or `Ctrl+Left` or `Ctrl+Right` | key | press | M1 | manual 42.19 |
| S-TRACK-013 | Arm the selected tracks for recording | `C` | `C` | key | press | M2 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-TRACK-014 | Solo the selected tracks | `S` | `S` | key | latch | M1 | manual 42.19, 42.25; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-TRACK-016 | Deactivate the selected track | `0` | `0` | key | press | M1 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-TRACK-017 | Freeze or unfreeze the selected tracks | `Cmd+Option+Shift+F` | `Ctrl+Alt+Shift+F` | key | press | M4 | manual 42.19; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and context menus |
| S-TRACK-018 | Delete the track from its title bar | `Delete` | `Delete` | key | press | M1 | manual 42.19 |

## take-lanes

A take lane is selected in the Arrangement.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-COMP-002 | Copy the selected take-lane area to the main lane | `Enter` | `Enter` | key | press | M6 | manual 42.17 |
| S-COMP-003 | Audition the selected take lane | `T` | `T` | key | press | M6 | manual 42.17 |
| S-COMP-005 | Duplicate the selected take lane | `Cmd+D` | `Ctrl+D` | key | press | M6 | manual 42.17 |
| S-COMP-006 | Replace the main lane's clip with the previous or next take | `Cmd+Up` or `Cmd+Down` | `Ctrl+Up` or `Ctrl+Down` | key | press | M6 | manual 42.17 |
| S-COMP-007 | Go from a take lane to its track's main lane and fold the take lanes | `Left` | `Left` | key | press | M6 | manual 21.1 |
| S-COMP-008 | Move the selected take lanes up or down | `Cmd+Up` or `Cmd+Down` | `Ctrl+Up` or `Ctrl+Down` | key | press | M6 | manual 21.2 |

## loop-brace

A loop brace or a clip start or end marker is selected, or a multi-clip loop bar.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-LOOP-005 | Move the start marker to the clicked position | Cmd+click | Ctrl+click | mouse | press | M2 | manual 42.8 |
| S-LOOP-006 | Move the end marker to the clicked position | Cmd+Shift+click | Ctrl+Shift+click | mouse | press | M2 | manual 42.8 |
| S-LOOP-007 | Nudge the loop brace left or right by the grid | `Left` or `Right` | `Left` or `Right` | key | press | M1 | manual 42.8 |
| S-LOOP-008 | Move the loop by its own length | `Down` or `Up` | `Down` or `Up` | key | press | M1 | manual 42.8 |
| S-LOOP-009 | Halve or double the loop length | `Cmd+Down` or `Cmd+Up` | `Ctrl+Down` or `Ctrl+Up` | key | press | M1 | manual 42.8 |
| S-LOOP-010 | Shorten or lengthen the loop by the grid | `Cmd+Left` or `Cmd+Right` | `Ctrl+Left` or `Ctrl+Right` | key | press | M1 | manual 42.8 |
| S-LOOP-011 | Select the material inside the loop | `Cmd+Shift+L` | `Ctrl+Shift+L` | key | press | M1 | manual 42.8 |
| S-SAMPLE-006 | Move the clip region with the start marker | `Shift+Left` or `Shift+Right` | `Shift+Left` or `Shift+Right` | key | press | M2 | manual 42.11 |
| S-NOTE-022 | Move the clip region with the start marker | `Shift+Left` or `Shift+Right` | `Shift+Left` or `Shift+Right` | key | press | M2 | manual 42.12 |
| S-ARR-012 | Shorten or lengthen the loop brace | `Cmd+Left` or `Cmd+Right` | `Ctrl+Left` or `Ctrl+Right` | key | press | M1 | manual 42.16 |
| S-LOOP-012 | Duplicate Loop: double the loop and copy its notes into the new half | `Cmd+D` | `Ctrl+D` | key | press | M6 | manual 10.7.3, 10.8 |

## envelope

An automation lane or the Clip View's Envelope Editor has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-EDIT-016 | Apply an editing command to the selected part of an envelope | Option (with the command) | Alt (with the command) | modifier | held | M2 | manual 42.5 |
| S-ENV-002 | Drag breakpoints in finer steps | Shift (hold while dragging) | Shift (hold while dragging) | modifier | held | M2 | manual 42.7 |
| S-ENV-003 | Curve an automation segment | Option (hold while dragging a segment) | Alt (hold while dragging a segment) | modifier | held | M2 | manual 42.7 |
| S-ENV-005 | Delete the selected envelope | `Cmd+Delete` | `Ctrl+Delete` | key | press | M2 | manual 42.7 |
| S-ENV-006 | Ignore grid snapping while dragging breakpoints | Cmd (hold while dragging) | Alt (hold while dragging) | modifier | held | M2 | manual 42.7 |
| S-A11Y-018 | Move the insert marker along an envelope | `Left` or `Right` | `Left` or `Right` | key | press | M6 | manual 41.5 |
| S-A11Y-019 | Create or select a breakpoint at the insert marker, or confirm an edit | `Enter` | `Enter` | key | press | M6 | manual 41.5 |
| S-A11Y-020 | Jump to the previous or next breakpoint | `Option+Left` or `Option+Right` | `Alt+Left` or `Alt+Right` | key | press | M6 | manual 41.5 |
| S-A11Y-021 | Select a breakpoint and move the insert marker to it | Option+click | Alt+click | mouse | press | M6 | manual 41.5 |
| S-A11Y-022 | Select the next or previous breakpoint | `Tab` or `Shift+Tab` | `Tab` or `Shift+Tab` | key | press | M6 | manual 41.5 |
| S-A11Y-023 | Change the selected breakpoint's value | `Up` or `Down` | `Up` or `Down` | key | press | M6 | manual 41.5 |
| S-A11Y-024 | Cancel the breakpoint edit and deselect | `Esc` | `Esc` | key | press | M6 | manual 41.5 |
| S-A11Y-025 | Delete the selected breakpoint | `Delete` | `Delete` | key | press | M6 | manual 41.5 |
| S-A11Y-028 | Cycle the automated parameters in the Clip View's Envelope Editor | `Option+Up` or `Option+Down` | `Alt+Up` or `Alt+Down` | key | press | M6 | Live 12 release notes, 12.2, bug fixes (the Clip View's Envelope Editor) |
| S-A11Y-029 | Cycle all of the clip's device parameters in the Clip View's Envelope Editor | `Shift+Option+Up` or `Shift+Option+Down` | `Shift+Alt+Up` or `Shift+Alt+Down` | key | press | M6 | Live 12 release notes, 12.2, bug fixes (the Clip View's Envelope Editor) |
| S-A11Y-030 | Move the selected breakpoint in time by the grid | `Left` or `Right` | `Left` or `Right` | key | press | M6 | manual 41.5.1; Live 12 release notes, 12.2 |

## clip-view

The Clip View has focus, whichever editor it shows.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-LOOP-001 | Set the clip start marker at the play position | `Cmd+F9` | `Ctrl+F9` | key | press | M2 | manual 42.8; Live 12 release notes, 12.0 |
| S-LOOP-002 | Set the loop start at the play position | `Cmd+F10` | `Ctrl+F10` | key | press | M2 | manual 42.8; Live 12 release notes, 12.0 |
| S-LOOP-003 | Set the loop end at the play position | `Cmd+F11` | `Ctrl+F11` | key | press | M2 | manual 42.8; Live 12 release notes, 12.0 |
| S-LOOP-004 | Set the clip end marker at the play position | `Cmd+F12` | `Ctrl+F12` | key | press | M2 | manual 42.8; Live 12 release notes, 12.0 |
| S-CLIPMODE-002 | Cycle the Notes, Envelopes and MPE tabs (unconfirmed) | `Option+Tab` | `Ctrl+Tab` | key | press | M3 | manual 42.10 |
| S-CLIPMODE-003 | Show the Content Editor (the Sample or Notes tab) | `Option+Shift+1` | `Alt+Shift+1` | key | press | M2 | manual 42.10; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-CLIPMODE-004 | Show the Envelope Editor (the Envelopes tab) | `Option+Shift+2` | `Alt+Shift+2` | key | press | M3 | manual 42.10; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-CLIPMODE-005 | Show the Expression Editor (the MPE tab) | `Option+Shift+3` | `Alt+Shift+3` | key | press | M6 | manual 42.10; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-NOTE-030 | Expand the Clip View to full size | `Cmd+Option+E` | `Ctrl+Alt+E` | key | press | M6 | manual 42.12; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-NOTE-052 | Choose the previous or next chord in Stacks' Chord Selector Pad | `Cmd+Up` or `Cmd+Down` | `Ctrl+Up` or `Ctrl+Down` | key | press | M6 | manual 11.3.4 |
| S-NOTE-053 | Select every breakpoint of the focused expression envelope | `Cmd+A` | `Ctrl+A` | key | press | M6 | manual 12.2 |

## sample-editor

The Clip View shows an audio clip's Sample Editor.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-CLIPMODE-001 | Switch between the Sample and Envelopes tabs (unconfirmed) | `Option+Tab` | `Ctrl+Tab` | key | press | M3 | manual 42.10 |
| S-SAMPLE-001 | Quantise the selection | `Cmd+U` | `Ctrl+U` | key | press | M4 | manual 42.11; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-SAMPLE-002 | Open the quantise settings | `Cmd+Shift+U` | `Ctrl+Shift+U` | key | press | M4 | manual 42.11; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-SAMPLE-003 | Move the selected warp marker | `Left` or `Right` | `Left` or `Right` | key | press | M4 | manual 42.11 |
| S-SAMPLE-004 | Select the previous or next warp marker | `Cmd+Left` or `Cmd+Right` | `Ctrl+Left` or `Ctrl+Right` | key | press | M4 | manual 42.11 |
| S-SAMPLE-005 | Scroll the display to follow playback (unconfirmed) | `Option+Shift+F` | `Ctrl+Shift+F` | key | press | M4 | manual 42.11 |
| S-SAMPLE-007 | Zoom to the clip selection | `Z` | `Z` | key | press | M4 | manual 42.11 |
| S-SAMPLE-008 | Zoom back from the clip selection | `X` | `X` | key | press | M4 | manual 42.11 |
| S-SAMPLE-009 | Insert a warp marker | `Cmd+I` | `Ctrl+I` | key | press | M4 | manual 42.11 |
| S-SAMPLE-010 | Delete the selected warp marker | `Delete` | `Delete` | key | press | M4 | manual 42.11 |
| S-SAMPLE-011 | Insert a transient | `Cmd+Shift+I` | `Ctrl+Shift+I` | key | press | M4 | manual 42.11 |
| S-SAMPLE-012 | Delete the selected transient (unconfirmed) | `Cmd+Shift+Delete` | `Ctrl+Shift+Delete` | key | press | M4 | manual 42.11 |
| S-SAMPLE-013 | Fit the content to the editor's width | `W` | `W` | key | press | M4 | manual 42.11 |
| S-SAMPLE-014 | Fit the content to the editor's height | `H` | `H` | key | press | M4 | manual 42.11 |
| S-A11Y-012 | Move the insert marker in the Sample Editor (unconfirmed) | `Option+Left` or `Option+Right` | `Ctrl+Left` or `Ctrl+Right` | key | press | M2 | manual 41.4.1.4 |
| S-A11Y-013 | Start or extend a time selection in the Sample Editor | `Shift+Left` or `Shift+Right` | `Shift+Left` or `Shift+Right` | key | press | M2 | manual 41.4.1.4 |

## note-editor

The Clip View shows a MIDI clip's MIDI Note Editor.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-NOTE-001 | Select all notes | `Cmd+A` | `Ctrl+A` | key | press | M2 | manual 42.12 |
| S-NOTE-002 | Copy notes by dragging | Option+drag | Ctrl+drag | mouse | press | M2 | manual 42.12 |
| S-NOTE-003 | Chop the selected notes on the grid, or split them at the time selection's edges | `Cmd+E` | `Ctrl+E` | key | press | M6 | manual 42.12 |
| S-NOTE-004 | Chop notes into a number of parts set by dragging up or down (unconfirmed) | Cmd+E (hold) and drag or Option+E (hold) and drag | Ctrl+E (hold) and drag | mouse | held | M6 | manual 42.12 |
| S-NOTE-005 | Chop notes in steps of two while dragging up or down | Cmd+Shift+E (hold) and drag or Option+Shift+E (hold) and drag | Ctrl+Shift+E (hold) and drag | mouse | held | M6 | manual 42.12 |
| S-NOTE-006 | Split a note where clicked; drag to move the split | E (hold) and click | E (hold) and click | mouse | held | M6 | manual 42.12 |
| S-NOTE-007 | Join the selected notes | `Cmd+J` | `Ctrl+J` | key | press | M6 | manual 42.12 |
| S-NOTE-008 | Fit the selected notes to the time range | `Cmd+Option+J` | `Ctrl+Alt+J` | key | press | M6 | manual 42.12; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-NOTE-009 | Fit the content to the editor's width | `W` | `W` | key | press | M2 | manual 42.12 |
| S-NOTE-010 | Fit the content to the editor's height | `H` | `H` | key | press | M2 | manual 42.12 |
| S-NOTE-011 | Quantise the selected notes | `Cmd+U` | `Ctrl+U` | key | press | M2 | manual 42.12; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-NOTE-012 | Open the quantise settings | `Cmd+Shift+U` | `Ctrl+Shift+U` | key | press | M2 | manual 42.12; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-NOTE-013 | Scroll the editor up or down | `PageUp` or `PageDown` | `PageUp` or `PageDown` | key | press | M2 | manual 42.12 |
| S-NOTE-014 | Scroll the editor up or down in small steps | `Shift+PageUp` or `Shift+PageDown` | `Shift+PageUp` or `Shift+PageDown` | key | press | M2 | manual 42.12 |
| S-NOTE-015 | Scroll the editor left or right | `Cmd+PageUp` or `Cmd+PageDown` | `Ctrl+PageUp` or `Ctrl+PageDown` | key | press | M2 | manual 42.12 |
| S-NOTE-016 | Change velocity by dragging a note up or down | Cmd+drag | Alt+drag | mouse | held | M2 | manual 42.12 |
| S-NOTE-017 | Move the insert marker to the start | `Home` or `Fn+Left` | `Home` | key | press | M2 | manual 42.12 |
| S-NOTE-018 | Move the insert marker to the end | `End` or `Fn+Right` | `End` | key | press | M2 | manual 42.12 |
| S-NOTE-019 | Select the previous or next note (unconfirmed) | `Option+Down` or `Option+Up` | `Alt+Down` or `Alt+Up` | key | press | M2 | manual 42.12 |
| S-NOTE-020 | Select the previous or next note of the same pitch (unconfirmed) | `Option+Left` or `Option+Right` | `Alt+Left` or `Alt+Right` | key | press | M2 | manual 42.12 |
| S-NOTE-021 | Scroll the display to follow playback (unconfirmed) | `Option+Shift+F` | `Alt+Shift+F` | key | press | M2 | manual 42.12, 42.9 |
| S-NOTE-023 | Zoom to the clip selection | `Z` | `Z` | key | press | M2 | manual 42.12 |
| S-NOTE-024 | Zoom back from the clip selection | `X` | `X` | key | press | M2 | manual 42.12 |
| S-NOTE-025 | Zoom in or out horizontally | `Plus` or `Minus` | `Plus` or `Minus` | key | press | M2 | manual 42.12 |
| S-NOTE-033 | Apply the current MIDI tool's settings | `Cmd+Enter` | `Ctrl+Enter` | key | press | M6 | manual 42.12; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-NOTE-034 | Invert the note selection | `Cmd+Shift+A` | `Ctrl+Shift+A` | key | press | M6 | manual 42.12; observed in 12.4.3 (macOS), 10 October 2026: Edit menu |
| S-NOTE-035 | Highlight the clip's scale | `K` | `K` | key | press | M6 | manual 42.12; observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-NOTE-036 | Split an Arrangement clip at the time selection | `Cmd+Shift+E` | `Ctrl+Shift+E` | key | press | M6 | manual 42.12 |
| S-NOTE-037 | Show or hide the note filters | `Cmd+Shift+F` | `Ctrl+Shift+F` | key | press | M6 | manual 42.12 |
| S-A11Y-004 | Select time in the MIDI Note Editor | `Shift+Left` or `Shift+Right` | `Shift+Left` or `Shift+Right` | key | press | M2 | manual 41.4.1.4 |
| S-A11Y-005 | Select the notes inside the time selection | `Enter` | `Enter` | key | press | M2 | manual 41.4.1.4 |
| S-A11Y-006 | Move the insert marker to the previous or next note edge (unconfirmed) | `Option+Left` or `Option+Right` | `Ctrl+Left` or `Ctrl+Right` | key | press | M2 | manual 41.4.1.4 |
| S-A11Y-007 | Extend the note selection while jumping between notes | Shift (with the note-jump keys) | Shift (with the note-jump keys) | modifier | held | M2 | manual 41.4.1.4 |
| S-NOTE-038 | Fold the editor to the key tracks that hold notes | `F` | `F` | key | press | M2 | manual 10.6; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-NOTE-039 | Fold the editor to the clip's scale | `G` | `G` | key | press | M6 | manual 10.6; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-NOTE-040 | Toggle Focus Mode for multi-clip editing (momentary while held) | `N` | `N` | key | latch | M6 | manual 10.8.1 |
| S-NOTE-044 | Turn Step Input Mode on or off | `Cmd+Shift+B` | `Ctrl+Shift+B` | key | press | M6 | observed in 12.4.3 (macOS), 10 October 2026: Options menu |
| S-NOTE-045 | Move the insert marker one grid step earlier or later | `Left` or `Right` | `Left` or `Right` | key | press | M2 | manual 10.5.2 |
| S-NOTE-046 | Move the time selection's end to the next or previous note start or end | `Option+Shift+Left` or `Option+Shift+Right` | `Alt+Shift+Left` or `Alt+Shift+Right` | key | press | M2 | manual 10.5.2 |
| S-NOTE-048 | Add the held notes at the insert marker and step on, or lengthen the notes just added | `Right` | `Right` | key | press | M6 | manual 19.3.4 |
| S-NOTE-049 | Shorten, then remove, the notes just added, and step back | `Left` | `Left` | key | press | M6 | manual 19.3.4 |

## note-selection

Notes are selected in the MIDI Note Editor.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-NOTE-026 | Set the selected notes' velocity by typing 0 to 127, then Enter | 0..9 then Enter | 0..9 then Enter | sequence | press | M2 | manual 42.12 |
| S-NOTE-027 | Lower or raise the selected notes' velocity | `Cmd+Down` or `Cmd+Up` | `Ctrl+Down` or `Ctrl+Up` | key | press | M2 | manual 42.12 |
| S-NOTE-028 | Lower or raise the selected notes' velocity deviation | `Cmd+Shift+Down` or `Cmd+Shift+Up` | `Ctrl+Shift+Down` or `Ctrl+Shift+Up` | key | press | M6 | manual 42.12 |
| S-NOTE-029 | Lower or raise the selected notes' chance | `Cmd+Option+Down` or `Cmd+Option+Up` | `Ctrl+Alt+Down` or `Ctrl+Alt+Up` | key | press | M6 | manual 42.12 |
| S-NOTE-031 | Group the selected notes (play all) | `Cmd+G` | `Ctrl+G` | key | press | M6 | manual 42.12 |
| S-NOTE-032 | Ungroup the selected notes | `Cmd+Shift+G` | `Ctrl+Shift+G` | key | press | M6 | manual 42.12 |
| S-A11Y-008 | Transpose the selected notes by a semitone | `Up` or `Down` | `Up` or `Down` | key | press | M2 | manual 41.4.1.4 |
| S-A11Y-009 | Transpose the selected notes by an octave | `Shift+Up` or `Shift+Down` | `Shift+Up` or `Shift+Down` | key | press | M2 | manual 41.4.1.4 |
| S-A11Y-010 | Shorten or lengthen the selected notes by the grid | `Shift+Left` or `Shift+Right` | `Shift+Left` or `Shift+Right` | key | press | M2 | manual 41.4.1.4 |
| S-A11Y-011 | Move the selected notes by the grid | `Left` or `Right` | `Left` or `Right` | key | press | M2 | manual 41.4.1.4 |
| S-NOTE-041 | Deactivate the selected notes | `0` | `0` | key | press | M6 | manual 10.5.7 |
| S-NOTE-042 | Deselect the notes | `Esc` | `Esc` | key | press | M2 | manual 10.5.2 |
| S-NOTE-043 | Add the previous or next note to the selection (unconfirmed) | `Option+Shift+Down` or `Option+Shift+Up` | `Ctrl+Shift+Down` or `Ctrl+Shift+Up` | key | press | M2 | manual 10.5.2 |
| S-NOTE-047 | After a chop, while Cmd stays held, chop the same notes into one part more or fewer, or with Shift into the next or previous power of two | `Cmd+Up` or `Cmd+Down` or `Cmd+Shift+Up` or `Cmd+Shift+Down` | `Ctrl+Up` or `Ctrl+Down` or `Ctrl+Shift+Up` or `Ctrl+Shift+Down` | key | press | M6 | manual 10.5.8.2 |
| S-NOTE-050 | Change the selected notes' velocity or release velocity by 10, or by 1 with Shift | `Cmd+Up` or `Cmd+Down` or `Cmd+Shift+Up` or `Cmd+Shift+Down` | `Ctrl+Up` or `Ctrl+Down` or `Ctrl+Shift+Up` or `Ctrl+Shift+Down` | key | press | M6 | manual 10.5.12 |
| S-NOTE-051 | Change the selected notes' chance by 10 %, or by 1 % with Shift | `Up` or `Down` or `Shift+Up` or `Shift+Down` | `Up` or `Down` or `Shift+Up` or `Shift+Down` | key | press | M6 | manual 10.5.13 |

## device-view

The Device View has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-DEV-001 | Group the selected devices into a rack | `Cmd+G` | `Ctrl+G` | key | press | M5 | manual 42.4; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and the device title bar's context menu |
| S-DEV-002 | Ungroup the selected rack | `Cmd+Shift+G` | `Ctrl+Shift+G` | key | press | M5 | manual 42.4 |
| S-DEV-003 | Turn every device in a rack on or off | Option+click a device activator | Alt+click a device activator | mouse | press | M5 | manual 42.4 |
| S-DEV-004 | Add devices to the selection | Shift+click | Shift+click | mouse | press | M2 | manual 42.4 |
| S-DEV-008 | Hot-swap the selected device | `Q` | `Q` | key | press | M5 | manual 42.4 |
| S-DEV-010 | Open another plug-in window without closing the others | Cmd+click a plug-in window button | Ctrl+click a plug-in window button | mouse | press | M2 | manual 42.4; observed in Live 12 (macOS), 12 October 2026: Info View text of the plug-in device's window button |
| S-DEV-011 | Switch the device between its A and B states | `P` | `P` | key | press | M5 | manual 42.4; observed in 12.4.3 (macOS), 10 October 2026: Edit menu and the device title bar's context menu |
| S-A11Y-014 | Move between devices | `Left` or `Right` | `Left` or `Right` | key | press | M2 | manual 41.4.1.5 |
| S-DEV-012 | Show the selected rack's chain list | `Plus` | `Plus` | key | press | M5 | observed in 12.4.3 (macOS), 10 October 2026: View menu (Show Chains) |
| S-DEV-013 | Hide the selected rack's chain list | `Minus` | `Minus` | key | press | M5 | observed in 12.4.3 (macOS), 10 October 2026: View menu (Hide Chains) |
| S-DEV-014 | Fold or unfold the selected device (unconfirmed) | `Minus` | `Minus` | key | press | M2 | observed in 12.4.3 (macOS), 10 October 2026: device title bar's context menu (Fold) |
| S-DEV-015 | Float the selected device in its own window, or dock it | `Option+F` | `Ctrl+Alt+F` | key | press | M2 | Beatlayer (IMP-14, WP-2.5b), decided 12 October 2026; not in Live |

## rack-chains

A rack's chain list has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-SES-023 | Solo the selected rack chain | `S` | `S` | key | press | M5 | manual 42.15 |

## sample-swap

A Drum Rack, a pad, Simpler or Drum Sampler is selected.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-SWAP-001 | Swap to the next similar sample | `Cmd+Right` | `Ctrl+Right` | key | press | M9 | manual 42.23 |
| S-SWAP-002 | Swap to the previous similar sample | `Cmd+Left` | `Ctrl+Left` | key | press | M9 | manual 42.23 |
| S-SWAP-003 | Keep the current sample as the similarity reference | `Cmd+Up` | `Ctrl+Up` | key | press | M9 | manual 42.23 |
| S-SWAP-004 | Return to the reference sample | `Cmd+Down` | `Ctrl+Down` | key | press | M9 | manual 42.23 |
| S-SWAP-005 | Show the swap controls on drum pads while held | Option | Alt | modifier | held | M9 | manual 42.23 |

## browser

The browser's content pane has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-DEV-007 | Load the selected device from the browser | `Enter` | `Enter` | key | press | M2 | manual 42.4 |
| S-TRACK-015 | Add the browser's selected device to the selected track | `Enter` | `Enter` | key | press | M2 | manual 42.19 |
| S-BRW-001 | Move down or up the list | `Down` or `Up` | `Down` or `Up` | key | press | M1 | manual 42.22 |
| S-BRW-003 | Close or open the selected folder | `Left` or `Right` | `Left` or `Right` | key | press | M1 | manual 42.22 |
| S-BRW-004 | Load the selected item | `Enter` | `Enter` | key | press | M1 | manual 42.22 |
| S-BRW-005 | Preview the selected file | `Shift+Enter` or `Right` | `Shift+Enter` or `Right` | key | press | M1 | manual 42.22 |
| S-BRW-008 | Give the selected items a collection colour | `1..7` | `1..7` | key | press | M6 | manual 42.22; observed in 12.4.3 (macOS), 10 October 2026: browser item's context menu |
| S-BRW-009 | Clear the selected items' collection colours | `0` | `0` | key | press | M6 | manual 42.22; observed in 12.4.3 (macOS), 10 October 2026: browser item's context menu |
| S-BRW-010 | Show files similar to the selected one | `Cmd+Shift+F` | `Ctrl+Shift+F` | key | press | M9 | manual 42.22; observed in 12.4.3 (macOS), 10 October 2026: View menu and the browser item's context menu |
| S-BRW-011 | Go back in the browser's history | `Cmd+LeftBracket` | `Ctrl+LeftBracket` | key | press | M6 | manual 42.22; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-BRW-012 | Go forward in the browser's history | `Cmd+RightBracket` | `Ctrl+RightBracket` | key | press | M6 | manual 42.22; observed in 12.4.3 (macOS), 10 October 2026: Navigate menu |
| S-BRW-013 | Show or hide the filter view | `Cmd+Option+G` | `Ctrl+Alt+G` | key | press | M6 | manual 42.22; observed in 12.4.3 (macOS), 10 October 2026: View menu |
| S-BRW-014 | Show or hide the tag editor | `Cmd+Shift+E` | `Ctrl+Shift+E` | key | press | M6 | manual 42.22 |
| S-A11Y-015 | Extend the selection in the content pane | `Shift+Up` or `Shift+Down` | `Shift+Up` or `Shift+Down` | key | press | M1 | manual 41.4.1.6 |
| S-BRW-015 | Move the focus to Quick Tags' Add field | `Cmd+E` | `Ctrl+E` | key | press | M6 | Live 12 release notes, 12.2 (Quick Tags) |

## browser-sidebar

The browser's sidebar has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-A11Y-016 | Move between the sidebar's labels | `Up` or `Down` | `Up` or `Down` | key | press | M1 | manual 41.4.1.6 |
| S-A11Y-017 | Move between the sidebar and the content pane | `Left` or `Right` | `Left` or `Right` | key | press | M1 | manual 41.4.1.6 |

## browser-search

The browser's search field has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-BRW-007 | Jump from the search field to the results | `Down` or `Enter` | `Down` or `Enter` | key | press | M1 | manual 42.22 |

## computer-midi-keyboard

The computer MIDI keyboard is on.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-MAP-004 | Shift the computer keyboard's octave down or up | `Z` or `X` | `Z` or `X` | key | press | M2 | manual 42.24 |
| S-MAP-005 | Lower or raise the computer keyboard's note velocity | `C` or `V` | `C` or `V` | key | press | M2 | manual 42.24 |
| S-MAP-006 | Play notes from the letter keys (white keys on the home row, black keys above) (unconfirmed) | `A` or `W` or `S` or `E` or `D` or `F` or `T` or `G` or `Y` or `H` or `U` or `J` or `K` or `O` or `L` or `P` | `A` or `W` or `S` or `E` or `D` or `F` or `T` or `G` or `Y` or `H` or `U` or `J` or `K` or `O` or `L` or `P` | key | held | M2 | manual 42.24 (figure) |
| S-MAP-007 | Use a single-letter shortcut while the computer keyboard is on | Shift (with the letter) | Shift (with the letter) | modifier | held | M2 | manual 42.24 |

## settings

The Settings window has focus.

| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |
|---|---|---|---|---|---|---|---|
| S-SET-001 | Move to the next option | `Tab` | `Tab` | key | press | M1 | manual 42.26.2, 41.1.2 |
| S-SET-002 | Move to the previous option, or back to the page list from the first option | `Shift+Tab` | `Shift+Tab` | key | press | M1 | manual 42.26.2, 41.1.2 |
| S-SET-003 | Change the focused option | `Down` or `Up` | `Down` or `Up` | key | press | M1 | manual 42.26.2, 41.1.2 |
| S-SET-004 | Toggle the focused switch | `Enter` | `Enter` | key | press | M1 | manual 42.26.2, 41.1.2 |
| S-SET-005 | Show the next settings page | `Option+Tab` | `Ctrl+Tab` | key | press | M1 | manual 42.26.2, 41.1.2 |
| S-SET-006 | Show the previous settings page | `Option+Shift+Tab` | `Ctrl+Shift+Tab` | key | press | M1 | manual 42.26.2, 41.1.2 |
| S-SET-007 | Close the Settings window | `Esc` | `Esc` | key | press | M1 | manual 42.26.2, 41.1.2 |

## Notes, shared keys, and unconfirmed bindings

| ID | Status | Note |
|---|---|---|
| S-VIEW-004 | note | With two windows open, moves the Clip or Device View to the other window (manual 8). |
| S-VIEW-008 | note | The key labelled ? on a US layout. Manual 2.2.2 calls it the ? key. |
| S-VIEW-009 | unconfirmed | Corrected from the manual's Cmd+Option+V / Ctrl+Alt+V: Live 12.4.3's View menu shows Cmd+Option+Shift+V, and its Edit menu shows Cmd+Option+V for Paste Bounced Audio (S-BOUNCE-002). The Windows binding is inferred from macOS. |
| S-VIEW-012 | note | Manual 17 gives Ctrl+Option+I on macOS; Live 12.4.6's Mixer Controls submenu shows Cmd+Option+I. |
| S-FOCUS-009 | note | In Beatlayer it goes to the shown panel of the group the focus was last in (detail views, R24). |
| S-FILE-006 | note | macOS only. |
| S-FILE-007 | note | Manual 41.4.1.3 gives Cmd+R / Ctrl+R for this, which is Rename everywhere else; treated as an error in that section. |
| S-FILE-008 | unconfirmed | Live 12.4.3 shows no key beside Export MIDI Clip... in the File menu or the clip context menus (observed 10 October 2026). |
| S-DEV-006 | same command as S-VAL-004 | Double-clicking the control also resets it. |
| S-DEV-007 | same command as S-BRW-004 |  |
| S-DEV-008 | same command as S-VIEW-006 |  |
| S-EDIT-004 | note | Duplicating a track puts the copy straight after it and selects it; one undo step, Duplicate Track(s) (pressed in 12.4.6 (macOS), 12 October 2026). |
| S-EDIT-007 | note | Windows uses Ctrl+Y, not Ctrl+Shift+Z. |
| S-VAL-001 | note | On the volume field Up and Down step 1 dB; from 5.5 dB, Up gives 6.0 dB and stops at the top. |
| S-VAL-002 | note | On the volume field Shift with Up and Down steps 0.1 dB. |
| S-VAL-003 | note | A drag of the pan knob with Shift held moves it about three times finer than a plain drag. |
| S-ENV-002 | same command as S-VAL-003 |  |
| S-ENV-005 | note | Manual 27.1 gives Ctrl+Backspace on Windows for clearing a clip envelope. |
| S-ENV-006 | same command as S-EDIT-013 |  |
| S-LOOP-001 | note | The 12.0 release notes say these set the markers to the time selection. |
| S-LOOP-002 | note | The 12.0 release notes say these set the markers to the time selection. |
| S-LOOP-003 | note | The 12.0 release notes say these set the markers to the time selection. |
| S-LOOP-004 | note | The 12.0 release notes say these set the markers to the time selection. |
| S-ZOOM-003 | note | Manual 41.4.1.3 describes the same keys as widening and narrowing the Arrangement grid; the adaptive grid follows the zoom. |
| S-ZOOM-005 | unconfirmed | Sections 42.11, 42.12 and 42.16 give Ctrl+Shift+F on Windows for the same command. |
| S-CLIPMODE-001 | unconfirmed | Manual 8.1.3 gives Alt+Tab on Windows. |
| S-CLIPMODE-002 | unconfirmed | Manual 8.1.3 gives Alt+Tab on Windows. |
| S-SAMPLE-005 | unconfirmed | Section 42.9 gives Alt+Shift+F on Windows for the same command. |
| S-SAMPLE-006 | note | Acts when the start marker is selected (42.8). |
| S-SAMPLE-012 | unconfirmed | Manual 9.2.3 gives Ctrl+Shift+Backspace on Windows. |
| S-NOTE-004 | unconfirmed | Manual 10.5.8 gives E with Option (macOS) or E with Ctrl (Windows) held while dragging; both macOS bindings are listed. |
| S-NOTE-019 | unconfirmed | Manual 41.4.1.4 gives Ctrl on Windows for the same command. |
| S-NOTE-020 | unconfirmed | Manual 41.4.1.4 gives Ctrl on Windows for the same command. |
| S-NOTE-021 | unconfirmed | On Windows 42.12 gives Ctrl+Shift+F, the same keys as Show or Hide Note Filters (S-NOTE-037) in the same editor, and 42.9 gives Alt+Shift+F. Beatlayer binds Alt+Shift+F and leaves Ctrl+Shift+F to the note filters. |
| S-NOTE-022 | same command as S-SAMPLE-006 |  |
| S-GRID-002 | note | Context-dependent keys: in the Arrangement, Cmd+1 to Cmd+4 are Narrow Grid, Widen Grid, Triplet Grid, and Snap to Grid (its clip and time-selection context menus); with the Session View focused, the Options menu lists the same keys as launch quantisation (S-SES-024 to S-SES-027), observed in 12.4.3. |
| S-GRID-003 | note | Context-dependent keys: in the Arrangement, Cmd+1 to Cmd+4 are Narrow Grid, Widen Grid, Triplet Grid, and Snap to Grid (its clip and time-selection context menus); with the Session View focused, the Options menu lists the same keys as launch quantisation (S-SES-024 to S-SES-027), observed in 12.4.3. |
| S-GRID-004 | note | Context-dependent keys: in the Arrangement, Cmd+1 to Cmd+4 are Narrow Grid, Widen Grid, Triplet Grid, and Snap to Grid (its clip and time-selection context menus); with the Session View focused, the Options menu lists the same keys as launch quantisation (S-SES-024 to S-SES-027), observed in 12.4.3. |
| S-GRID-005 | note | Context-dependent keys: in the Arrangement, Cmd+1 to Cmd+4 are Narrow Grid, Widen Grid, Triplet Grid, and Snap to Grid (its clip and time-selection context menus); with the Session View focused, the Options menu lists the same keys as launch quantisation (S-SES-024 to S-SES-027), observed in 12.4.3. In the Clip View's MPE tab, Cmd+4 switches that tab's own grid, saved with the clip (manual 12.2). |
| S-GRID-007 | same command as S-EDIT-013 |  |
| S-SES-001 | note | With a scene selected in the Main track, Enter launches the scene (41.4.1.2). |
| S-SES-010 | same command as S-SES-002 |  |
| S-SES-012 | same command as S-TRANS-008 |  |
| S-SES-022 | same command as S-SES-021 | The same key as the previous row: a second press moves from the last track to the scene. |
| S-ARR-005 | unconfirmed | Manual 6.7 gives Ctrl+Shift on Windows. |
| S-ARR-010 | same command as S-ENV-004 |  |
| S-ARR-012 | same command as S-LOOP-010 | Placed in the loop-brace context because 42.8 says the brace must be selected. |
| S-ARR-020 | note | Live 12.4.3's View menu shows U beside Show Selected Grouped Track. |
| S-ARR-021 | note | Live 12.4.3's View menu shows these keys beside Show Grouped Tracks. |
| S-ARR-022 | note | It makes selected take lanes taller or shorter too (manual 21.2). |
| S-ARR-023 | unconfirmed | Section 42.9 gives Alt+Shift+F on Windows for the same command. |
| S-ARR-024 | same command as S-ZOOM-007 |  |
| S-ARR-028 | note | With nothing selected the same keys move the insert marker by the grid (41.4.1.3). The Move Clips With Arrow Keys setting turns nudging off. |
| S-ARR-029 | note | Live 12.4.3 offers Reverse Clip(s) with this key for an empty MIDI clip too; the manual describes it for audio. |
| S-ARR-035 | note | Puts the keyboard focus in the mixer at the selected track; Right then moves the selection to the next track (observed in 12.4.6 (macOS), 11 October 2026). |
| S-COMP-006 | shares keys with S-COMP-008 | The manual gives Cmd+Up and Cmd+Down both for moving take lanes (21.2, S-COMP-008) and for replacing the main lane's material with the next or previous take (21.6, 42.17): with take lane headers selected they move lanes, with clips or time selected they replace. |
| S-BOUNCE-002 | note | Also works in a Session slot (shown in the empty slot's context menu). On macOS Live 12.4.3 moved the video window to Cmd+Option+Shift+V (S-VIEW-009), so these keys aren't shared there; the manual gives Ctrl+Alt+V for both on Windows. |
| S-TRACK-001 | note | Inserts the track directly after the selected track and selects it (pressed in 12.4.3 (macOS), 13 October 2026, with one track selected). |
| S-TRACK-002 | note | Inserts the track after the last selected track and selects it (pressed in 12.4.3 (macOS), 13 October 2026, with tracks 1 to 3 selected). |
| S-TRACK-004 | same command as S-EDIT-008 |  |
| S-TRACK-005 | same command as S-EDIT-012 |  |
| S-TRACK-006 | note | Inserts a group track named Group where the first selected track was, so the members follow it and are renumbered. The group takes the first member's colour and each member keeps its own. Each member's Audio To becomes the group and the group's is Main. The group is unfolded and selected afterwards. One undo step, Undo Group Tracks (pressed in 12.4.3 (macOS), 13 October 2026, with tracks 3 and 4 selected). |
| S-TRACK-007 | note | Removes the group track; its members return to its place in order, with Audio To set back to Main, and the first former member is selected. One undo step, Undo Ungroup Tracks (pressed in 12.4.3 (macOS), 13 October 2026). |
| S-TRACK-010 | same command as S-ARR-020 |  |
| S-TRACK-015 | same command as S-BRW-004 |  |
| S-TRACK-016 | note | Live 12.4.3's Edit menu reads Deactivate Track with this key while an empty slot of the track is selected. |
| S-TRANS-003 | note | Live 12.4.3's Playback menu shows these keys beside Play From Insert Marker (S-ARR-032), with the Session View focused. |
| S-TRANS-004 | same command as S-TRANS-001 | Same key as Play: with a time selection, playback starts from it. |
| S-TRANS-005 | note | Live 12.4.3's Playback menu shows no key beside Return Play Position to 1.1.1 (M-PLAY-004). |
| S-TRANS-010 | note | The keys count group tracks by their number: with a group at position 3, F3 turned the group's activator on or off (pressed in 12.4.6 (macOS), 11 October 2026). |
| S-BRW-002 | same command as S-VIEW-010 |  |
| S-BRW-003 | shares keys with S-BRW-005 | Right opens a folder; on a file it previews it instead. |
| S-BRW-005 | shares keys with S-BRW-003 |  |
| S-BRW-006 | note | Works from anywhere in the window and selects the All label (41.4.1.6). |
| S-MAP-006 | unconfirmed | The manual shows the layout as a picture. |
| S-MAP-007 | note | For example Shift+S solos the selected track. |
| S-SET-007 | same command as S-VIEW-020 |  |
| S-MENU-001 | unconfirmed | Windows only. The list is the menu titles observed on macOS in 12.4.3; Playback (Alt+P) is inferred, as the manual predates that menu. |
| S-MENU-005 | note | macOS only: the system Help menu search (Cmd+?). |
| S-CTX-002 | note | On macOS this is VoiceOver's VO+Shift+M. |
| S-A11Y-006 | unconfirmed; shares keys with S-NOTE-020 | On macOS the manual gives the same keys as Select the Previous or Next Note of the Same Pitch. |
| S-A11Y-012 | unconfirmed; shares keys with S-SAMPLE-004 | On Windows the manual gives the same keys as Select the Previous or Next Warp Marker. |
| S-A11Y-023 | note | Typing a number also sets the value. |
| S-NOTE-043 | unconfirmed; shares keys with S-NOTE-028 | On Windows the manual gives the same keys as Lower or Raise the Selected Notes' Velocity Deviation (42.12). |
| S-FILE-009 | note | Not in the manual. The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-FILE-010 | note | macOS only: a standard item of the application menu. |
| S-VIEW-022 | note | The menu names the view the detail area shows: Hide Device View with the Device View open. Reading it as the detail-area toggle is ours. The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-VIEW-023 | note | Max-specific in Live; Beatlayer's extension host takes the place in M9 or drops it (M-VIEW-030). The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-DEV-013 | shares keys with S-DEV-014 | Live shows the same key beside Fold in the device title bar's context menu (S-DEV-014). |
| S-DEV-014 | unconfirmed; shares keys with S-DEV-013 | Live shows the minus key beside Fold in the device's context menu and beside Hide Chains in the View menu (S-DEV-013). |
| S-DEV-015 | note | Beatlayer's own key for Float, its item after Fold in the device's context menu. Scoped by focus: only with the Device View or a floating device's window focused and a device selected. On Windows and Linux it's Ctrl+Alt+F, which with the Arrangement focused stays Create Fade/Crossfade (S-ARR-008); Alt+F alone opens the File menu there (S-MENU-001). On macOS Option+F, beside Cmd+Option+F for fades. |
| S-SES-024 | note | Context-dependent keys: listed in the Options menu with the Session View focused; in the Arrangement the same keys are the grid commands (S-GRID-002 to S-GRID-005). The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-SES-025 | note | Context-dependent keys: listed in the Options menu with the Session View focused; in the Arrangement the same keys are the grid commands (S-GRID-002 to S-GRID-005). The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-SES-026 | note | Context-dependent keys: listed in the Options menu with the Session View focused; in the Arrangement the same keys are the grid commands (S-GRID-002 to S-GRID-005). The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-SES-027 | note | Context-dependent keys: listed in the Options menu with the Session View focused; in the Arrangement the same keys are the grid commands (S-GRID-002 to S-GRID-005). The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-SES-028 | same command as S-ARR-003 |  |
| S-ARR-037 | same command as S-SES-007 |  |
| S-NOTE-044 | note | Disabled in the Options menu in a new set. The Windows binding is inferred from macOS (Cmd to Ctrl, Option to Alt). |
| S-MENU-006 | note | macOS only: the system adds this item to every Edit menu (M-EDIT-040). |
| S-NOTE-045 | note | With no notes selected; with notes selected the arrows move them (S-A11Y-011). |
| S-NOTE-047 | shares keys with S-NOTE-027, S-NOTE-028, S-NOTE-043 | Only straight after Cmd+E (S-NOTE-003) while Cmd stays held; otherwise the same keys change velocity and velocity deviation, and on Windows Ctrl+Shift+Up and Down also add the next or previous note to the selection (S-NOTE-043). |
| S-NOTE-048 | shares keys with S-NOTE-045 | Only while step recording: the note editor shows a clip on an armed track with the Preview switch on and the transport stopped. |
| S-NOTE-049 | shares keys with S-NOTE-045 | Only while step recording: the note editor shows a clip on an armed track with the Preview switch on and the transport stopped. |
| S-NOTE-050 | shares keys with S-NOTE-027, S-NOTE-028, S-NOTE-043, S-NOTE-047 | Only after a marker in the Velocity or Release Velocity lane was clicked last. In the Velocity and Release Velocity lanes. The steps of 1 with Shift are Beatlayer's own. |
| S-NOTE-051 | shares keys with S-A11Y-008, S-A11Y-009 | Only after a marker in the Chance lane was clicked last. In the Chance lane. The steps of 1 % with Shift are Beatlayer's own. |
| S-NOTE-052 | note | Handled by the Chord Selector Pad while it has the keyboard focus. |
| S-NOTE-053 | note | In the MPE tab, on the focused expression envelope. |
| S-LOOP-012 | note | With the loop brace selected, or with multi-clip loop bars selected. |
| S-COMP-008 | shares keys with S-COMP-006 | The manual gives Cmd+Up and Cmd+Down both for moving take lanes (21.2) and for replacing the main lane's material with the next or previous take (21.6, 42.17, S-COMP-006): with take lane headers selected they move lanes, with clips or time selected they replace. |
| S-A11Y-028 | same command as S-A11Y-026 |  |
| S-A11Y-029 | same command as S-A11Y-027 |  |
| S-A11Y-030 | shares keys with S-A11Y-018 | While a breakpoint is selected; with none selected, Left and Right move the insert marker (S-A11Y-018). |
| S-MENU-007 | note | Beatlayer's own key for Help ▸ Search Commands…, which Live 12 leaves unbound on both platforms; it works wherever the keyboard focus is. |
| S-FOCUS-015 | note | Beatlayer's own key for Navigate ▸ Find Track, which Live 12 leaves unbound on both platforms; it works wherever the keyboard focus is. |

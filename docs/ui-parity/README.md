# UI parity reference

Beatlayer puts every view, panel, control, menu item, context menu, and shortcut where Ableton Live 12 puts it, so a Live user has nothing to learn (decision D16 in [the architecture](../02-architecture.md#decisions-log)). This folder is the specification for that: every UI package is built against it and checked against it. Built in WP-0.13.

It records function and placement only. It holds no Ableton artwork, icons, screenshots, or manual text ([clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code), "Layout and shortcuts"). Live's names for views, commands, and devices appear only to identify what Beatlayer matches; Beatlayer's devices get their own names.

## Files

| File | What it holds | IDs |
|---|---|---|
| [layout.md](layout.md) | Every view and panel: position, size relative to its neighbours, what shows or hides it, and its controls in order | `L-AREA-NNN` |
| [shortcuts.toml](shortcuts.toml) | Every keyboard shortcut, with macOS and Windows bindings, context, trigger, source, and milestone. The source of truth for the default keymap | `S-AREA-NNN` |
| [shortcuts.md](shortcuts.md) | The same shortcuts as tables by context, generated from shortcuts.toml. Don't edit it by hand | |
| [menus.md](menus.md) | Every menu-bar menu with its items in order, and the main context menus | `M-MENU-NNN`, `C-MENU-NNN` |
| [interactions.md](interactions.md) | Mouse gestures and modifiers, drop targets, keyboard focus, held keys | `I-AREA-NNN` |

## How UI packages use it

1. **Cite rows.** A UI package's pull request lists the layout rows (L-), menu rows (M-, C-), interaction rows (I-), and shortcuts (S-) it implements. A row is done when its placement, controls, order, and behaviour match.
2. **Compare side by side at 1600 × 1000.** Set Beatlayer's window and Live's window to 1600 × 1000 points (outer frame), put them side by side in the same state (the "default state" in layout.md, or the state the row describes), and compare positions, proportions, and order. Paste the comparison notes in the PR. Live's screenshots are never committed.
3. **Bind the shortcuts.** Every shortcut whose milestone is the package's milestone, in a context the package builds, is bound in the default keymap (`crates/shell-spec/keymaps/live12.toml`, generated from shortcuts.toml). The keymap test fails if a shortcut is neither bound nor marked with a later milestone.
4. **Differences need a decision.** A deliberate difference from a row (for example a Live menu item that names an Ableton service) needs its own entry in the decisions log, and the row gets a note pointing at it.

## Sources

- **Live 12.4.3 on macOS**, observed on 10 October 2026 by reading its interface and by using its menus and controls, as the Method below describes. A row that rests on a later observation gives its Live version and date.
- **The Live 12 reference manual** as published on 10 October 2026, mainly chapters 2 (First Steps, including Settings), 3 (Live Concepts), 4 (browser), 6 (Arrangement View), 7 (Session View), 8 (Clip View), 18 (Mixing), 23 (instruments and effects), 41 (accessibility and keyboard navigation), and 42 (keyboard shortcuts). Everything taken from it is paraphrased. A source written `manual 42.12` means chapter 42, section 12.

Every row says which source it rests on: `obs` (observed in Live, with its version and date where it isn't 12.4.3 on 10 October 2026) or `man N` (manual section), or both.

Windows bindings are as the manual gives them. A shortcut added from a macOS menu carries a Windows binding made by swapping Cmd for Ctrl and Option for Alt.

## Method

- **Window.** Measurements are for Live's main window at 1600 × 1000 points (its outer frame, title bar included), with the Learn View both open and closed.
- **Accessibility tree.** Element names, roles, and frames come from macOS's accessibility interface. Live 12's tree names most controls in the Control Bar, browser, mixer, Clip View panels, and Settings window; it doesn't expose the Info View, the detail area's empty states, the Status Bar, or the view selectors, which are measured from captures instead.
- **Screen captures** are used only to measure positions and read on-screen order, then discarded. Coordinates are converted from capture pixels to points (1600 points across the window).
- **Menus.** The titles and order of every menu and submenu come from the menu bar's accessibility interface. The separators of each menu-bar menu, the keys it shows, and which items are disabled or ticked come from opening it in a new set. Every key a menu shows is matched to an entry in shortcuts.toml, which confirms it on macOS.
- **Context menus** are those of eight objects in a new set: an empty Session slot, a MIDI track's title bar, a scene, a browser preset, a Session clip, a device title bar, an Arrangement clip, and an Arrangement time selection. Live draws these menus itself.
- **Behaviour** that the behaviour specs rely on is described there: the Undo History's entries after mixer drags, clip inserts, and time-signature edits; tempo against the time signature; bar numbering before 1.1.1; and fragmentary bars ([musical time](../specs/musical-time.md), [tracks and undo](../specs/tracks-and-undo.md)).
- **Manual.** Chapter 42's 292 table rows are transcribed one by one into shortcuts.toml (in our own words), and the rest of the manual is searched for shortcuts and gestures outside that chapter.

## shortcuts.toml

Each `[[shortcut]]` has:

| Field | Meaning |
|---|---|
| `id` | Stable ID, `S-AREA-NNN`. Never renumber or reuse one; to retire a shortcut, leave a note in the PR. |
| `action` | What it does, in our own words. |
| `context` | Where it applies; one of the `[[context]]` entries (`global`, `session`, `arrangement`, `note-editor`, and so on). The same keys may do different things in different contexts. |
| `mac`, `win` | Bindings as the manual gives them for each platform, as lists of alternatives. Key chords use `+` (`Cmd+Shift+S`, `Ctrl+Alt+V`); macOS modifiers are Fn, Ctrl, Option, Shift, Cmd and Windows modifiers Ctrl, Alt, Shift. Key names: A to Z, 0 to 9, F1 to F12, Tab, Enter, Esc, Space, Delete, Backspace, Home, End, PageUp, PageDown, Up, Down, Left, Right, Plus, Minus, Comma, Period, Slash, LeftBracket, RightBracket, Menu. Ranges are written `F1..F8`. An empty list means the platform has no binding. |
| `kind` | `key` (a key chord, checked for conflicts), `mouse` (a modifier with a click or drag), `modifier` (a modifier held during another action), or `sequence` (several steps, such as typing a value then Enter). |
| `trigger` | `press` (acts once when pressed), `held` (acts while held), or `latch` (toggles when tapped and acts only while held if held for about half a second, as Live 12's momentary shortcuts do). |
| `milestone` | The Beatlayer milestone that binds it (M1 to M10), from the [parity matrix](../04-parity-matrix.md). |
| `source` | Manual section(s), and `observed in 12.4.3 (macOS), 10 October 2026: <where>` where the binding was seen in Live, naming the menu or context menu that shows it (or `pressed`). |
| `same_as` | Optional: the ID of another entry for the same command (the manual lists some commands in several sections). |
| `allow_conflict` | Optional: IDs that bind the same keys in the same context, where the manual itself does so. Always with a note. |
| `confirmed` | Optional. `true` where Live 12.4.3 showed the macOS binding (its `source` says where); `false` when the sources disagree or the binding needs checking; left out for a binding taken from the manual alone. |
| `notes` | Optional explanation. |

To change a shortcut, edit shortcuts.toml, run `cargo xtask ui-parity` to regenerate shortcuts.md, and commit both. `cargo xtask check` (and `cargo test`) fail if:

- shortcuts.md differs from what shortcuts.toml generates;
- an ID is duplicated or malformed, a field is unknown or misspelt, or a context, kind, trigger, or milestone isn't one of the allowed values;
- a binding uses an unknown key or the wrong platform's modifier;
- two `key` entries bind the same chord in the same context on the same platform, unless one names the other in `same_as` or `allow_conflict`;
- a page in this folder cites a row ID (S-, L-, M-, C-, I-) that no file defines, or defines one twice.

## What the reference covers

- **Shortcuts:** 359 entries. All 292 rows of the manual's shortcut chapter (42), each with both platforms' bindings, including the momentary keys (42.25) as `latch` triggers. Then 51 more from the manual: 27 from the accessibility chapter (41), 7 Settings window keys (42.26.2), 5 menu keys (42.28), 2 context-menu keys (42.29), 2 from the text of 42.24, and 8 found elsewhere. Then 15 that Live 12.4.3's menus show and the manual doesn't list, among them the launch quantisation keys the Options menu gives ⌘1 to ⌘4 with the Session View focused. 152 entries rest on observation, each naming in its `source` where the binding was seen. One more is Beatlayer's own, not Live's: S-DEV-015, Float (IMP-14, WP-2.5b), on Option+F on macOS and Ctrl+Alt+F on Windows and Linux, in the Device View only (Ctrl+Alt+F stays the Arrangement's fade key, S-ARR-008). Counts by milestone and context are in shortcuts.md.
- **Layout:** 171 rows covering every view and panel the manual names: the window regions, Control Bar, browser, Session View, Arrangement View, mixer sections, Clip View, Device View, Groove Pool, Info View, Learn View, Status Bar, second window, Settings window and its ten pages, and the secondary windows (Undo History, File Manager, export dialog, video window, plug-in windows, map modes, MPE settings).
- **Menus:** all nine menu-bar menus of 12.4.3 on macOS (the application menu, File, Edit, Create, Playback, View, Navigate, Options, Help) in 237 rows including submenus, with their 62 separators, the keys shown on 119 items, and the items disabled (71) or ticked (9) in a new set; then, observed in 12.4.6 on 11 October 2026, the Mixer Controls and Arrangement Track Controls submenus, adding 3 separators (65 in all), the keys on 3 more items, and their ticks; then the Control Bar's Quantization and Metronome Settings menus, in 32 rows. Nine context menus (the crossfader's added), in 128 rows with 42 separators, each followed by the commands the manual adds in other states (30 rows).
- **Interactions:** 115 rows of clicks, drags, scrolling, drop targets, focus movement, and held keys.

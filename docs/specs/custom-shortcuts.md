# Your own keyboard shortcuts

- **Status:** implemented (R1 to R17 each have a passing test)
- **Work packages:** WP-6.43 (R1 to R17: the user keymap, the Shortcuts page, and their commands)
- **Parity-matrix row:** Interface: Keyboard shortcuts, macOS and Windows, including held (momentary) shortcuts; [the improvements](../05-improvements.md), IMP-17 (your own keyboard shortcuts)
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026; the Live 12 release notes, 12.0 to 12.4.6; the UI parity reference's [shortcuts.toml](../ui-parity/shortcuts.toml) and [layout.md](../ui-parity/layout.md) L-SET-002, L-SET-003 and L-SET-010

## The behaviour

Beatlayer's keyboard shortcuts are Live 12's, and they stay the default. A person who wants another key for a command, or a key for a command Live gives none, sets it on the Shortcuts page in Settings: they pick the command, press the keys, and the new shortcut works at once, everywhere the command's menu item and the command search show their keys. If the keys already run something else in the same part of the interface, the page says what and lets them replace it or cancel. Reset puts one command, or every command, back to Live's keys.

What a person changes is kept in a file of their own, `keymap.toml`, beside Beatlayer's other preferences, layered over the generated Live 12 keymap (`live12.toml`), which never changes. A person with no file has Live's keymap exactly. The file is plain text, so it can be copied to another computer or shared. Live allows no custom shortcuts; its Key Map Mode, which maps keys to controls in a set, is a different thing and stays milestone M7's.

This spec covers the user keymap file, how it layers over the default keymap, what can and can't be rebound, the Shortcuts page, conflicts, reset, and the commands an agent uses. The default bindings are [command-bindings.md](command-bindings.md)'s and the shortcut reference's; how a key press finds its command is [app-shell.md](app-shell.md) R11.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Beatlayer | [The improvements](../05-improvements.md), IMP-17, and rules 1 to 4 | A user keymap file layered over `live12.toml`; a Shortcuts page in Settings to rebind any command, warn on conflicts within a context, and reset to Live's; Live's keymap stays the default |
| S2 | Manual | Live 12 reference manual, chapter 42 "Live Keyboard Shortcuts", and chapter 34 "MIDI and Key Remote Control"; chapter 3, section 3.20 | The shortcuts are fixed; computer keys can be mapped to a set's controls in Key Map Mode, which is saved with the set. No chapter describes changing a shortcut |
| S3 | Release notes | Live 12 release notes, 12.0 to 12.4.6, read 20 October 2026 | Shortcuts are added and changed by releases (Copy Time and S for Solo in 12.4, for example); none lets a person set one; the fixes to custom key mappings concern Key Map Mode |
| S4 | Reference | [shortcuts.toml](../ui-parity/shortcuts.toml), its contexts and fields, and the keymap generated from it (`crates/shell-spec/keymaps/live12.toml`) | Each shortcut has an ID, both platforms' keys, one context, a kind (key, mouse, modifier, sequence) and a trigger (press, held, latch); the keymap marks each bound to its commands, handled by the system or a focused control, or pending |
| S5 | Reference | [layout.md](../ui-parity/layout.md) L-SET-002 (the Settings pages and their order, Agents a Beatlayer addition after Plug-Ins), L-SET-003 (a page's layout) and L-SET-010 (Display & Input, with its Navigation and Keyboard section) | Where Live keeps its keyboard settings, and how a Beatlayer page is added |
| S6 | Beatlayer | [app-shell.md](app-shell.md) R5, R11 and R15; [command-bindings.md](command-bindings.md); [command-palette.md](command-palette.md) R5; [accessibility.md](accessibility.md) | The Settings window's pages; the order in which contexts resolve a key; the menus show their keys; every action reaches assistive technology |
| S7 | Beatlayer | [The architecture](../02-architecture.md), "Layout, commands, and key bindings" and "Agent control"; decisions D16 and D17 | The default keymap is generated data and a check fails if it drifts; every action is a registered command with a permission class and a readback |

## Rules

**The user keymap**

- **R1.** A person's own shortcuts are in `keymap.toml` in Beatlayer's preferences folder, beside `preferences.json`. Without the file, or with an empty one, the keymap is the default keymap exactly: every key does what `live12.toml` says. The default keymap file is never written. (S1, S7)
- **R2.** The file holds `schema = 1` and two kinds of entry. A `[[bind]]` entry names a command by its ID, any arguments it fixes (as the default keymap's entries fix them), its keys on macOS (`mac`) and on Windows and Linux (`win`), each a list written as `shortcuts.toml` writes keys, and optionally its context; without one, it takes the context of the command's default shortcut, or `global` when it has none. An `[[unbind]]` entry names a default shortcut by its ID (`S-EDIT-004`), and optionally one platform, and takes that shortcut's keys away. The format is Beatlayer's own. (S1, S4)
- **R3.** The keymap in use is the default keymap without the shortcuts the file unbinds, with every bind entry added. A bind entry's keys win over a default shortcut with the same keys in the same context, whose keys go for as long as the entry stays; the default shortcut's command keeps its other keys. A command can have several shortcuts. (S1, S4)
- **R4.** Keys resolve as they always do ([app-shell.md](app-shell.md) R11): the focused area's context, then `tab-focus` while Use Tab Key to Move Focus is on, then `global`. A user shortcut keeps its command's trigger: a command whose default shortcut acts while held, or latches (S4), does so on its new keys too. (S4, S6)
- **R5.** Only key shortcuts can be rebound. Mouse gestures, modifiers held during a gesture, and sequences keep Live's; so do the shortcuts the keymap marks as the system's or a focused control's (menu navigation, Hide, the value and text field keys), and the `text-field`, `value-field`, `menu`, `dialog` and `computer-midi-keyboard` contexts take no user shortcut, so typing a name or a value, and playing the computer keyboard, always work as in Live. (S4, S6)
- **R6.** An entry the file holds but can't use (a command ID the registry doesn't have, keys that don't parse, a context the keymap doesn't have, a context R5 refuses, a shortcut ID that doesn't exist) is skipped and the rest load. At launch the Status Bar says how many entries were skipped, and `keymap.state` lists each with why. A skipped entry stays in the file until the person changes or resets it. (S1)

**The Shortcuts page**

- **R7.** Settings has a Shortcuts page, straight after Display & Input in the page chooser (L-SET-002), laid out as Live's pages are (L-SET-003). It lists every command a key can run, by the menu or context it belongs to, as the command search names them ([command-palette.md](command-palette.md) R4, R5): its label, its context, and its keys on this platform, a person's own shown as theirs. A search field at the top narrows the list by the label, as the command search matches it. The page's name and place are Beatlayer's own. (S1, S5)
- **R8.** Selecting a row and clicking its keys, or pressing Return, starts recording: the cell reads "Press keys", and the next key pressed with its modifiers becomes the command's shortcut in its context, replacing the person's own shortcut for it if it had one. Esc stops recording and changes nothing; the keys R5 refuses are refused with a message. A Remove button beside the keys takes away the shortcut shown, default or the person's own. (S1)
- **R9.** When the keys pressed already run another command in the same context, or in `global` for a narrower one, or in a narrower context for a `global` one, the page names that command and its context and offers Replace, which moves the keys to the new command, and Cancel, which changes nothing. Nothing is saved until one is chosen. Keys that run another command only in an unrelated context (the Session View's and the note editor's) are allowed without a warning. (S1, S4)
- **R10.** A command whose keys differ from Live's shows a Reset button in its row, which gives it back exactly its default shortcuts. Reset All, at the page's foot, asks first and then empties the file, so the keymap is Live's again. Wherever Beatlayer shows a command's keys (the menu bar, the context menus, the command search, the Info View's descriptions), it shows the keys in use, so a reset or a change shows there at once. (S1, S6)
- **R11.** A change takes effect at once and is written to the file at once. It isn't part of the set and isn't an undo step: Cmd-Z never undoes a shortcut. (S1)
- **R12.** Every part of the page works from the keyboard and reaches assistive technology: the list is a table whose rows read the command's label, context and keys; recording announces "Press keys", and a conflict's question is read as a dialog. (S6)
- **R13.** At every interface zoom the page scales as the other Settings pages do. (S6)

**Commands**

- **R14.** `keymap.state` (read) reports the keymap in use on each platform: every shortcut with its keys, context, trigger and commands, and whether it's Live's, a person's own, or a default one the file unbinds; and each entry R6 skipped, with why. (S7)
- **R15.** `keymap.bind` (command, arguments, keys, platform, optional context), `keymap.unbind` (a shortcut's ID, or a command and keys) and `keymap.reset` (a command, or every command) are registered commands in the `edit` class, each read back through `keymap.state`; `keymap.bind` refuses the keys R5 refuses and, when the keys run another command where R9 would warn, refuses unless `replace` is true. A person's gesture on the page runs one of them. (S1, S7)
- **R16.** The default keymap's own test still holds: with no user file, every shortcut in the reference is bound or marked with the milestone that binds it, and the keymap is the one `cargo xtask keymap` generates. (S7)
- **R17.** Key Map Mode's mappings (milestone M7) are the set's and aren't shortcuts: the user keymap neither holds nor changes them. (S2)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/shell/tests/keymap.rs`, `with_no_file_the_keymap_is_lives`, `an_empty_file_leaves_the_keymap_alone` | unit |
| R2 | `crates/shell/tests/keymap.rs`, `an_unbind_takes_the_keys_away_on_one_platform_only`, `a_bind_keeps_the_arguments_it_fixes` | unit |
| R3 | `crates/shell/tests/keymap.rs`, `a_bind_gives_a_command_other_keys`, `a_bind_over_lives_own_key_takes_it`, `a_replace_keeps_the_other_keys_of_lives_own_shortcut` | unit |
| R4 | `crates/shell/tests/keymap.rs`, `a_latching_command_keeps_its_latch_on_new_keys` | unit |
| R5 | `crates/shell/tests/keymap.rs`, `entries_that_cant_be_used_are_skipped_and_listed` (the refused context) | unit |
| R6 | `crates/shell/tests/keymap.rs`, `entries_that_cant_be_used_are_skipped_and_listed` | unit |
| R7 | `crates/ui/tests/user_keymap.rs`, `the_shortcuts_page_sits_after_display_and_input` | UI script |
| R8 | `crates/ui/tests/user_keymap.rs`, `recording_sets_a_key` | UI script |
| R9 | `crates/ui/tests/user_keymap.rs`, `a_conflict_asks_and_replace_moves_the_key` | UI script |
| R10 | `crates/ui/tests/user_keymap.rs`, `reset_and_reset_all_give_lives_keys_back` | UI script |
| R11 | `crates/shell/tests/keymap.rs`, `undo_leaves_the_keymap_alone` | unit |
| R12 | `crates/ui/tests/user_keymap.rs`, `every_row_reaches_accesskit` | UI script |
| R13 | `crates/ui/tests/zoom.rs`, `the_settings_window_draws_its_rows_scaled_at_every_step` | UI script |
| R14 | `crates/shell/tests/keymap.rs`, `the_commands_read_back_through_keymap_state`; `crates/cli/tests/agent.rs`, `an_agent_binds_keys_and_reads_them_back` | unit, through the agent API |
| R15 | `crates/shell/tests/keymap.rs`, `the_commands_read_back_through_keymap_state`, `a_reset_gives_the_command_lives_keys_back`; `crates/cli/tests/agent.rs`, `an_agent_binds_keys_and_reads_them_back` | unit, through the agent API |
| R16 | `crates/ui/tests/keymap.rs`, `every_shortcut_is_bound_or_marked_with_the_milestone_that_binds_it` | unit |
| R17 | `crates/shell/tests/keymap.rs`, `entries_that_cant_be_used_are_skipped_and_listed` (the file holds no Key Map Mode mapping, and one it held would be skipped) | unit |

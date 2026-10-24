# The Undo History: the panel, its list, and jumping back and forward through it

- **Status:** implemented (R1 to R12 each have a passing test)
- **Work packages:** WP-6.28 (R1 to R12). Undo and redo themselves, one step per edit and what a step is, are [tracks-and-undo.md](tracks-and-undo.md)'s and the editor's (WP-0.6)
- **Parity-matrix row:** Views and workflow: Undo History panel
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026; the Live 12 release notes, 12.1 and 12.3; the UI parity reference's rows L-OTH-001, L-WIN-007, L-WIN-008, L-WIN-009, L-LRN-002, M-VIEW-008, M-EDIT-001 and M-EDIT-002, and S-VIEW-021, recorded from Live 12.4.3 on macOS

## The behaviour

The Undo History lists everything done to the set since it was opened or made, newest first, so a person can go back or forward several steps at once instead of pressing Undo again and again. It opens from View ▸ Undo History or Cmd+Option+Z as a panel on the right of the main window, in the column the Learn View uses. Each row is named for its step as the Edit menu's Undo names it, and the bottom row is the set's start, which can't be undone. Clicking a row, or moving to it with the arrow keys and pressing Enter, returns the set to that point: the rows above it turn grey, as undone, and stay listed until a new edit replaces them; clicking a grey row puts its steps back. The history isn't saved with the set.

Beatlayer adds what Live has no need for: a step an agent made carries the agent's name and mark, so the person sees which edits were theirs and which an agent's, and agents can jump through the history by command as the person does.

This spec covers the panel, its list, its keys, the jump, and what an agent sees and does. What counts as a step, and how undo and redo behave, is [tracks-and-undo.md](tracks-and-undo.md)'s.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 5 "Managing Files and Sets", section 5.4.2 "Accessing a Set's Undo History" | View ▸ Undo History, or Cmd+Option+Z (Ctrl+Alt+Z), shows the actions done since the set was opened, newest at the top and oldest at the bottom, for undoing or redoing several at once. Clicking an entry, or moving to it with Up and Down and pressing Enter, returns the set to that point: the entries above it are greyed as undone and those below stay active; selecting a greyed entry reapplies it and the undone entries beneath it. The history isn't saved with the set and starts again each time it's opened. Opening or making the set is the first entry and can't be undone |
| S2 | Release notes | Live 12 release notes, 12.1 ("Undo History") and 12.3 | 12.1: the Undo History lists the undo and redo steps there are, and steps back or forward through several at once by a click or by the arrow keys and Enter; it's in the View menu, with Cmd+Option+Z (Ctrl+Alt+Z). Changes made through mapped keys each make an entry of their own. 12.3: after closing views such as the Undo History, the focus goes back to the main view |
| S3 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in [layout.md](../ui-parity/layout.md) L-OTH-001, L-WIN-007 to L-WIN-009 and L-LRN-002, [menus.md](../ui-parity/menus.md) M-VIEW-008, M-EDIT-001 and M-EDIT-002, and [tracks-and-undo.md](tracks-and-undo.md), items 10 to 13 | The Undo History is a panel in the right-hand column where the Learn View sits, not a window of its own, 295 points wide at 1600 × 1000, from the Control Bar's bottom to the window's bottom edge, the Status Bar stopping at its left. It lists the steps newest first, each named for its action (Change "Track Volume", Insert Clip), with New Live Set at the bottom; undone steps stay listed, greyed, above the current one until a new edit replaces them. The Learn View's header has its close button at the left and its title after it. Undo and Redo read Nothing to Undo and Nothing to Redo, and otherwise name the step |
| S4 | Manual | Same manual, chapter 42, through [shortcuts.toml](../ui-parity/shortcuts.toml) S-VIEW-021 | Cmd+Option+Z (Ctrl+Alt+Z) opens the Undo History, in every context |
| S5 | Beatlayer | [The architecture](../02-architecture.md), "The document" and "Agent control", D17, D21 and D24; [tracks-and-undo.md](tracks-and-undo.md); [app-shell.md](app-shell.md) R1, R2 and R23; [interface-zoom.md](interface-zoom.md) R1; [accessibility.md](accessibility.md) R1 and R2 | The undo stack keeps each step's earlier version of the document; a gesture is one step, committed on release; a new or opened set starts with no history; agent edits are transactions named with the agent's name and show in the undo history; agent calls wait while a person's gesture is under way; Beatlayer's name where Live names its own product; the assistant's pane at the right edge; every command has a schema, a description and a permission class; heights and widths scale with the zoom |

## Rules

**The panel**

- **R1.** View ▸ Undo History (M-VIEW-008), Cmd+Option+Z (S-VIEW-021), `view.toggle_undo_history`, shows the Undo History panel in the main window's right-hand column, the Learn View's place (L-OTH-001, L-WIN-008): 295 points wide at 1600 × 1000, from the Control Bar's bottom to the window's bottom edge, with the main view, the detail area and the Status Bar stopping 9 points left of it (L-WIN-007, L-WIN-009). Run again, or the close button at the left of the panel's header, which holds the title "Undo History" after it (as the Learn View's header does, L-LRN-002), hides it. The menu item is ticked while the panel shows (Beatlayer's own). (S1, S2, S3, S4)
- **R2.** While the assistant's pane is docked and open (D24), the Undo History sits immediately left of it, 9 points apart, the pane keeping the window's right edge; the main view, the detail area and the Status Bar stop 9 points left of the Undo History (Beatlayer's own). (S5)
- **R3.** Whether the panel shows is view state: it isn't saved with the set, isn't an undo step, and lasts the session; a new session starts with it hidden (Beatlayer's own). (S3, S5)

**The list**

- **R4.** The panel lists the set's history newest first, one row per step, each named by the step's label, the name Edit ▸ Undo or Edit ▸ Redo gives the step (M-EDIT-001, M-EDIT-002). The steps that can be redone come first, drawn in the dimmed text colour; then the steps that can be undone, the newest of them the current row, drawn selected; then a bottom row for the set's start, which reads "New Beatlayer Set" for a set made new (Live's "New Live Set", D21) and "Open Beatlayer Set" for one opened from a file (Beatlayer's own), and which no jump can undo. Each row is 18 points tall at 100%, its label cut short with an ellipsis where it's wider than the panel and shown whole as hover text (Beatlayer's own). (S1, S3, S5)
- **R5.** The list follows the history as it changes: a committed edit, an undo, a redo, a jump, and an agent's edit show at once, and the list scrolls to keep the current row in view. A gesture under way (a drag) shows only when it's committed, as one row. New Set and Open Set clear the list to its bottom row ([app-shell.md](app-shell.md) R1, R2). The history keeps every step since the set was opened or made, and the list scrolls through them (Beatlayer's own). (S1, S3, S5)

**Jumping**

- **R6.** Clicking a row returns the set to the state just after that row's step: the active steps above it are undone, newest first, and the greyed steps down to it are redone, oldest first, as one jump through `edit.go_to_step`, whose `index` counts rows up from the bottom row (0 is the set's start). Clicking the bottom row undoes every step; clicking the current row changes nothing. (S1, S2, S3)
- **R7.** A jump isn't a step of its own. After it, the rows above the clicked one are greyed and stay listed, Edit ▸ Undo and Redo name the steps either side of it, Cmd+Z and Cmd+Shift+Z go on from there one step at a time, and the next edit removes the greyed rows ([tracks-and-undo.md](tracks-and-undo.md): a new edit discards the steps that could be redone). (S1, S3, S5)
- **R8.** While the panel has the keyboard focus, Up and Down move a highlight from row to row without changing the set, and Enter jumps to the highlighted row as a click does. Opening the panel by its menu item or key gives it the focus with the current row highlighted, and clicking a row gives it the focus (Beatlayer's own). Esc, and hiding the panel while it has the focus, give the focus back to the main view. (S1, S2)

**Agents**

- **R9.** The editor records with each step who made it: the person, or an agent by the name the agent API knows it by, or the built-in assistant. A row for a step an agent or the assistant made is drawn with an agent mark before its label (Beatlayer's own mark), and its hover text and its accessible description say "Made by <name>". The person jumps past an agent's steps as past their own. (S5)
- **R10.** `view.toggle_undo_history` (`read`, view state) and `edit.go_to_step` (`edit`, as `edit.undo` is) are registered commands with schemas and descriptions, run by ID through the agent API; an agent's jump waits while a person's gesture is under way, as every agent call does (architecture, "Agent control"). `edit.go_to_step` reads back through the query tool's `song` object, whose `undo` field gives `steps`, `redo_steps`, `next_undo` and `next_redo`, and through its `history` path, which lists every row from the top, with its label, whether it's undone, who made it, and the current row's index. `view.state` reports whether the panel shows ([app-shell.md](app-shell.md) R23). (S5)

**Size and assistive technology**

- **R11.** The panel's width, rows and text scale with the interface zoom ([interface-zoom.md](interface-zoom.md) R1). (S5)
- **R12.** The accessibility tree holds the panel as a region named "Undo History" with a list box inside; each row is a list item named by its label, its description naming the agent for an agent's step, reported as undone while greyed, and the current row selected; the highlight R8 moves is the list's active descendant, so a screen reader reads each row as it's reached. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/ui/tests/undo_history.rs`, `the_panel_opens_by_its_key_and_closes_by_it_again`, `the_menu_item_shows_the_panel_and_the_close_button_hides_it`, `the_panel_sits_in_the_right_hand_column`; `crates/shell/tests/undo_history.rs`, `the_panels_switch_is_view_state_and_no_undo_step` | UI script, unit |
| R2 | `crates/ui/tests/undo_history.rs`, `the_panel_sits_in_the_right_hand_column` (the panel is immediately left of the assistant's pane) | UI script |
| R3 | `crates/shell/tests/undo_history.rs`, `the_panels_switch_is_view_state_and_no_undo_step` | unit |
| R4 | `crates/ui/tests/undo_history.rs`, `the_accessibility_tree_holds_the_list_and_its_rows` (the rows, the current one selected, and the set's start at the bottom); `crates/shell/tests/undo_history.rs`, `a_jump_returns_the_set_to_its_point_and_is_no_step` (the rows' labels and their makers) | UI script, unit |
| R5 | `crates/ui/tests/undo_history.rs`, `the_accessibility_tree_holds_the_list_and_its_rows` (the list follows the history) | UI script |
| R6 | `crates/model/src/editor.rs`, `a_jump_walks_the_history_without_recording_a_step`; `crates/model/tests/undo_properties.rs`, `a_jump_gives_back_each_version`; `crates/ui/tests/undo_history.rs`, `clicking_a_row_returns_the_set_to_that_point` | unit, property, UI script |
| R7 | `crates/model/src/editor.rs`, `a_new_edit_after_a_jump_discards_the_greyed_steps`; `crates/shell/tests/undo_history.rs`, `a_new_edit_after_a_jump_discards_the_greyed_steps` | unit |
| R8 | `crates/ui/tests/undo_history.rs`, `the_keys_move_the_highlight_and_enter_jumps_to_it` | UI script |
| R9 | `crates/model/src/editor.rs`, `a_steps_maker_survives_undo_and_redo`; `crates/shell/tests/undo_history.rs`, `an_agents_step_says_who_made_it`, `an_assistants_turn_is_one_row_marked_with_the_assistant`; `crates/agent/tests/undo_history.rs`, `an_agents_batch_is_one_row_marked_with_the_agents_name`; `crates/ui/tests/undo_history.rs`, `the_accessibility_tree_holds_the_list_and_its_rows` | unit, UI script |
| R10 | `crates/model/tests/command_registry.rs`, `every_command_runs_by_id` (the jump by ID); `crates/shell/tests/undo_history.rs`, `a_jump_returns_the_set_to_its_point_and_is_no_step`; `crates/agent/tests/undo_history.rs`, `an_agent_jumps_through_the_history_and_reads_it_back`, `an_agents_jump_waits_for_the_persons_gesture`; `crates/commands/src/readback/arrangement.rs` (`edit.go_to_step` reads back through the song's `undo.current`) | unit, UI script |
| R11 | `crates/ui/tests/undo_history.rs`, `the_panel_scales_with_the_zoom` | UI script |
| R12 | `crates/ui/tests/undo_history.rs`, `the_accessibility_tree_holds_the_list_and_its_rows` (the region, the list box, the rows' names, their undone state, and the agent's description) | UI script |

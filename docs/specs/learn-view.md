# The Learn View: lessons in the right-hand column, Beatlayer's bundled lessons, and Show Me links

- **Status:** draft
- **Work packages:** WP-10.23 (the Learn View, its pages, keys, menu items and commands, and the lesson format: R1 to R19); WP-10.24 (Beatlayer's own lessons, their pictures and Show Me targets: R20 to R23). The right-hand column's other panels are [the Undo History](undo-history.md)'s and [crash recovery](crash-recovery.md) R12's (the save history); the demo set lessons may open is [the Help menu](help-menu.md) R15's
- **Parity-matrix row:** Views and workflow: Learn View (built-in lessons)
- **Checked against:** the Live 12 reference manual (online edition), chapter 2 section 2.2.1 "Learn View", read 30 October 2026; the UI parity reference's rows L-LRN-001 to L-LRN-005, L-WIN-008, L-OTH-001, M-VIEW-005, M-HELP-001, M-NAV-008, S-VIEW-018 and S-FOCUS-008, recorded from Live 12.4.3 on macOS

## The behaviour

The Learn View holds lessons beside the set, in the column at the right of the main window. Its home page has an introduction, buttons that filter the lessons by topic, and modules, each a list of lessons with a check mark on those completed. A lesson page has text and pictures, links to other lessons and to the manual, and Complete Lesson at its foot. A header row carries a close button, the page's title, and Home, Back and Forward. The Learn View opens by itself the first time Beatlayer starts, opens and closes from View ▸ Learn View, Help ▸ Learn View and Cmd-Option-7, takes the keyboard focus with Option-7, and comes back on the page it last showed.

Beatlayer's lessons are its own, bundled with the app and readable without a network; there is no Reload. Until Beatlayer plays video, they are text and pictures, with no video player and no picture-in-picture window. Beatlayer adds Show Me links, which flash the control a lesson talks about and give it the keyboard focus. The column shows one of the Learn View, the Undo History and the save history at a time; opening one replaces the one showing.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 2 "First Steps", section 2.2.1 "Learn View" | The Learn View shows by default the first time Live opens, closes with the X at its top left, and reopens from the Help menu or Cmd-Option-7 (Ctrl-Alt-7), on the last page visited while Live has stayed open. Its home page groups lessons in modules, filters them by topic, marks completed lessons with a check, and links to the manual at its foot. A lesson has a video, a short text, links to other lessons at its foot, a link back to the lessons at its top, and Complete Lesson at its foot. The video has a picture-in-picture window that stays on top. The Learn View needs a network connection, and Reload refreshes it |
| S2 | Reference | [layout.md](../ui-parity/layout.md) L-LRN-001 to L-LRN-005, L-WIN-008 and L-OTH-001 | The column's place (x 1296 to 1591, from 61 pt to the window's bottom at 1600 × 1000); the header row (close at the left, title, then Home, Back and Forward at the right, y 61 to 82); the home page; the lesson page; the picture-in-picture window; the Undo History in the same place |
| S3 | Reference | [menus.md](../ui-parity/menus.md) M-VIEW-005 (Learn View, ⌥⌘7, ticked), M-HELP-001 (Learn View, no key shown), M-NAV-008 (Learn View, ⌥7); [shortcuts.toml](../ui-parity/shortcuts.toml) S-VIEW-018 (Cmd-Option-7, Ctrl-Alt-7) and S-FOCUS-008 (Option-7, Alt-7) | The menu items and keys |
| S4 | Spec | [The Undo History](undo-history.md) R1 to R3, R11 and R13; [crash recovery](crash-recovery.md) R12; the architecture's D24 (the assistant's pane at the right); [accessibility](accessibility.md); [interface zoom](interface-zoom.md) R1 | The column's width and neighbours, the assistant's pane keeping the right edge, view state, the zoom |
| S5 | Beatlayer | [The improvements](../05-improvements.md), rules 1 to 4; `AGENTS.md` (no third-party content) | An addition never moves what Live users know; Beatlayer's content is its own |

## Rules

### The panel (WP-10.23)

- **R1.** View ▸ Learn View (M-VIEW-005), Help ▸ Learn View (M-HELP-001), Cmd-Option-7 or Ctrl-Alt-7 (S-VIEW-018) and `view.toggle_learn_view` show and hide the Learn View in the main window's right-hand column (L-LRN-001, L-WIN-008): 295 points wide at 1600 × 1000, from the Control Bar's bottom to the window's bottom edge, the main view, the detail area and the Status Bar stopping 9 points left of it, as the Undo History does ([the Undo History](undo-history.md) R1). The View menu's item is ticked while it shows. (S1, S2, S3)
- **R2.** The column holds one of the Learn View, the Undo History and the save history at a time: showing one hides the one showing ([the Undo History](undo-history.md) R13). With the assistant's pane docked and open, the Learn View sits immediately left of it, as the Undo History does ([the Undo History](undo-history.md) R2). (S2, S4)
- **R3.** Option-7 or Alt-7 (S-FOCUS-008, M-NAV-008) and `focus.learn_view` give the Learn View the keyboard focus, showing it first if it's hidden. (S3)
- **R4.** The header row (L-LRN-002), 21 points tall, holds a close button at its left, the page's title, and Home, Back and Forward at its right. The close button hides the Learn View. (S1, S2)
- **R5.** Home shows the home page, Back the page shown before, and Forward the page Back left, as a browser's history moves; Back and Forward are disabled where there's nowhere to go. Opening a page from a link after going Back drops the pages Forward would have shown. The history lasts while Beatlayer runs. (S2)
- **R6.** The home page (L-LRN-003) shows an introduction, then a row of topic filter buttons (All, then one per topic the lessons name, in the lessons' order), then each module with its title and its lessons in order, each a link with a check mark when it's complete, then links to the manual ([the Help menu](help-menu.md) R2). A topic filter shows only the lessons of that topic, and only the modules that have one; All, chosen at first, shows every lesson. One filter is chosen at a time. (S1, S2)
- **R7.** A lesson page (L-LRN-004) shows a Lessons link back to the home page at its top, then the lesson's title, its text and pictures, and, at its foot, Complete Lesson, then the module's next lesson as a link and the other lessons it links to. Complete Lesson marks the lesson complete and reads Lesson Completed, with a check, while it is; clicking it then marks the lesson incomplete again. (S1, S2)
- **R8.** Lessons have no video player and no picture-in-picture window (the video of L-LRN-004, and L-LRN-005) until Beatlayer plays video. (S1, S5)
- **R9.** A link in a lesson opens another lesson in the Learn View (`lesson:<id>`), a page of the bundled manual in the system's browser (`manual:<chapter>#<section>`, [the Help menu](help-menu.md) R2), or the demo set (`demo:`, [the Help menu](help-menu.md) R15). A link to a web address is shown as text, not followed, so lessons never reach the network. (S5)
- **R10.** A Show Me link (`show:<layout ID>`, such as `show:L-CTRL-002`) flashes the control with that layout ID, outlining it three times over 1.5 seconds in the theme's focus colour, and gives it the keyboard focus. It runs no command: when the control isn't drawn, the Status Bar says so and nothing changes. (S5)
- **R11.** The first time Beatlayer starts on a computer (with no preferences yet), the Learn View shows, on its home page. Later starts show it only if it was showing when Beatlayer last quit. (S1)
- **R12.** Hidden and shown again while Beatlayer runs, the Learn View comes back on the page it last showed, scrolled where it was. (S1)
- **R13.** Which lessons are complete is kept in the preferences, by lesson ID, and lasts across sessions; `learn.complete` with a lesson and `complete` sets it. (S1)
- **R14.** The lessons are bundled with the app and nothing is fetched; there is no Reload button. (S1, S5)
- **R15.** A lesson is a Markdown file, `lessons/<module>/<nn>-<slug>.md`, with front matter between `+++` lines holding its `id`, `title`, `topics` and `order`; each module's folder has a `module.toml` with its `id`, `title` and `order`, and `lessons/home.md` is the home page's introduction. The Learn View draws headings, paragraphs, lists, emphasis, strong text, inline code, pictures (palette PNG files beside the lessons) and links, and shows anything else as plain text. A lesson that doesn't parse isn't listed, and `learn.state` names it with the reason. `cargo xtask package` bundles the folder. (S5)
- **R16.** The Learn View's text and pictures scale with the interface zoom ([interface zoom](interface-zoom.md) R1), in Beatlayer's typeface and theme. (S4)
- **R17.** In the accessibility tree the Learn View is a region named Learn View; its header's buttons are named Close Learn View, Home, Back and Forward; the filters are toggle buttons named by their topics; each module is a heading with a list of links, each naming whether the lesson is complete; a lesson's text is a document a screen reader reads in order; Complete Lesson is a toggle button. (S4)
- **R18.** Every action is a registered command, run by ID through the agent API: `view.toggle_learn_view`, `focus.learn_view`, `learn.open` (a page: `home`, or a lesson's ID), `learn.back`, `learn.forward` and `learn.filter` (a topic, or `all`), all `read`, as view state; `learn.complete` (`edit`, a kept preference); and `learn.state` (`read`), which reports whether the Learn View shows, its page, its history and place in it, the filter, every module with its lessons and whether each is complete, and the lessons that didn't parse. Each command that isn't `read` names its readback. (S4)
- **R19.** Whether the Learn View shows, its page and its history are view state: not saved with the set, never an undo step. (S4)

### Beatlayer's lessons (WP-10.24)

- **R20.** Beatlayer's lessons are its own, written for Beatlayer, short and task-first, in modules: Getting Started, Session View, Arrangement, Warping, Devices and Racks, Mixing, and Working with Agents. They describe only what Beatlayer does. (S5)
- **R21.** Every picture in a lesson is made by `cargo xtask lesson-pictures` from a UI script, in a fixed theme and size, so the pictures follow the interface; running it again on the same platform gives the same files. (S5)
- **R22.** Every Show Me link names a layout ID that the interface draws in the state its lesson sets up. (S2, S5)
- **R23.** A lesson's links all resolve: every `lesson:` to a lesson that exists, every `manual:` to a manual page and section that exist. (S5)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 to R5 | to be written in WP-10.23 | UI script; keymap test (S-VIEW-018, S-FOCUS-008); menu test |
| R6, R7, R9, R10 | to be written in WP-10.23 | UI script |
| R11 to R13 | to be written in WP-10.23 | shell test with real preference files; UI script |
| R14, R15 | to be written in WP-10.23 | unit; `cargo xtask package` |
| R16, R17 | to be written in WP-10.23 | UI script; accessibility tree |
| R18, R19 | to be written in WP-10.23 | agent API, `cargo xtask commands` |
| R20 to R23 | to be written in WP-10.24 | lesson parse test; Show Me target test; picture regeneration |

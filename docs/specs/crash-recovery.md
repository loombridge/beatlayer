# Crash recovery and Save Continuously

- **Status:** partial (R1 to R11, R21 to R28 each have a passing test; R12 to R20 are WP-6.39's)
- **Work packages:** WP-1.11 (the journal, crash recovery, the Save Continuously option, and their commands: R1 to R11). The recovery prompt and the switch in the Settings window are the app shell's to draw (WP-1.7 and the Settings pages after it). WP-6.62 puts the service in the app: the shell journals every edit, the recovery prompt at launch, and the Save Continuously switch (R21 to R28). WP-6.39 builds the save history, which lists the save points and saved versions and restores them (R12 to R20).
- **Parity-matrix row:** Files and export: Crash recovery; (Beatlayer only) Save Continuously
- **Checked against:** the Live 12 reference manual (online edition) and the Live 12 release notes, read on 10 October 2026; for R12 to R28, the manual's chapter 5 again and the Live 12 release notes, read 20 October 2026, and the UI parity reference's menus.md (M-FILE-008, M-VIEW-008) and layout.md (L-WIN-008, L-OTH-001) rows.

## The behaviour

You're working on a set and Live stops: it crashes, you force it to quit, the power goes. The next time you start Live, it offers to bring the set back as it was when it stopped, including the edits you hadn't saved. Say yes and you carry on; say no and Live starts afresh, keeping the recovery files aside for a while in case you change your mind.

Beatlayer does the same, always: while a set is open, every finished edit is written to a journal beside the set within a second, and a set that was open when Beatlayer stopped without closing it is offered at the next launch, exactly as it was after its last journalled edit. Beatlayer adds one option Live doesn't have (IMP-05): **Save Continuously**, off by default. With it on, every finished edit of a set that has a file is saved within a second, so reopening the set shows it without any question, and the set keeps a history of save points since its last normal save. Save (Cmd-S) works as it always does.

This spec covers when edits reach the disk, when a set is offered, what recovering and declining do, what several running instances do, the Save Continuously option, and the save history: a list of the set's save points and saved versions, newest first, any of which a person restores as one undo step. How the prompt looks is the app shell's.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 5, "Managing Files and Sets", sections 5.4.1, "Creating, Opening and Saving Sets", and 5.4.2, "Accessing a Set's Undo History" | Save Live Set saves the set as it is; Save Live Set As and Save a Copy save it elsewhere. The undo history isn't saved with a set: it starts again each time the set is opened, and opening the set is its first entry. The manual has no section on crash recovery: no heading in its table of contents, and nothing in chapter 5, mentions a crash or recovering work. |
| S2 | Release notes | Live 12 release notes: 12.1 | Crash recovery works when several instances of Live of different versions run at once. When another instance of the same version is running, recovery is turned off and a message says so. |
| S3 | Release notes | Live 12 release notes: 12.2 | Fixed: Live wrongly reported another running instance, which stopped a set being recovered after a crash. |
| S4 | Release notes | Live 12 release notes: 12.3.5 | Files in the Crash folder in Live's preferences that are more than a week old are deleted when Live starts. |
| S5 | Release notes | Live 12 release notes: 12.3, 12.4, 12.4.1, 12.4.3 | Several fixes to crashes while a set was being restored after a crash; the one in 12.3 concerns a restored set that had two undo steps. Recovery restores a set, and seems to bring its undo steps with it. |
| S6 | Support article | Ableton Help, "Recovering a Set manually after a crash" (help.ableton.com, article 115001878844), read through a search engine's summary on 10 October 2026; the page itself refused the fetch | Recovery is started on the next launch by an undo file Live keeps in its preferences folder, and a dialog asks whether to restore. Answering no moves the recovery files into a Crash folder in the preferences folder, from which they can still be recovered by hand with the same Live version. |
| S7 | Beatlayer | [Improvements](../05-improvements.md), IMP-05; [the architecture](../02-architecture.md), "Project files and interop" | Save Continuously: every finished edit journalled to disk within a second, a history of save points, crash recovery always on whatever the setting, off by default as Live has no such mode. The journal sits beside the set, each entry naming the revision it follows; a normal save writes the next revision and retires the journal, so the set format stays as it is. A new setting goes where Live keeps related settings. |
| S8 | Reference | [menus.md](../ui-parity/menus.md) M-FILE-008 (Open Live Set Backups) and M-VIEW-008 (Undo History); [layout.md](../ui-parity/layout.md) L-WIN-008 and L-OTH-001 (the Undo History opens in the right-hand area where the Learn View sits, newest action first, a click going back to it) | Where Live keeps a set's older versions and its history, and how its history lists them |
| S9 | Beatlayer | [app-shell.md](app-shell.md) R3 and R19 (a save keeps the version it replaces in the project's `Backup` folder); [tracks-and-undo.md](tracks-and-undo.md); [accessibility.md](accessibility.md); [the architecture](../02-architecture.md), "Agent control" | A set's backups and where they are; every edit is one undo step; every control reaches assistive technology; every action is a registered command with a permission class and a readback |
| S10 | Beatlayer | [app-shell.md](app-shell.md) R1, R4 and R23; [layout.md](../ui-parity/layout.md) L-SET-015 (File & Folder); [accessibility.md](accessibility.md); [the architecture](../02-architecture.md), "Processes and threads", "Agent control" and "Testing" | The set the app starts with; New, Open and Quit ask about unsaved changes and an agent's command never asks; `settings.state` reports the preferences; the File & Folder page's rows; every dialog reaches assistive technology and the keyboard; nothing slow runs on the UI or audio thread; every action is a registered command reachable through the agent API |

## Rules

- **R1.** While a set is open, every finished edit (a command, a gesture's transaction once committed, an undo, a redo) is on disk within a second, on a background thread, whatever the Save Continuously setting. Crash recovery can't be turned off. (S6, S7)
- **R2.** When Beatlayer starts after a session ended without closing its set (a crash, a forced quit, a power cut), and the set had journalled edits after its last save, the set is offered for recovery: its file (or none, for a set never saved), its name, how many journal entries follow its save, when the last was made, its label, and whether the set file has been saved since by something else. Several are offered newest first. A session that ended with nothing unsaved leaves nothing to offer. (S2, S5, S6)
- **R3.** Recovering opens the set exactly as it was after its last journalled edit (nothing earlier than any edit the session knew was on disk), at its file, or untitled if it was never saved. It opens unsaved, with an empty undo history. It's journalled again straight away, so a second crash before it's saved offers it again. (S5, S6)
- **R4.** Declining removes the set's unsaved edits from beside it, so it isn't offered again, and keeps a copy of the journal in the recovery folder's `Discarded` folder. Copies more than seven days old are deleted when Beatlayer starts. (S4, S6)
- **R5.** A set open in another running instance of Beatlayer is never offered. Each instance's recovery works whatever other instances are running: none is ever turned off because another runs. (S2, S3)
- **R6.** Saving retires the set's journal, since the set file now holds every edit. Closing a set, saved or with its changes discarded, removes its unsaved edits, so nothing is offered at the next launch. (S1, S7)
- **R7.** Opening or recovering another set while the open one has unsaved edits that weren't saved or discarded keeps those edits recoverable: they're offered at once. (Beatlayer)
- **R8.** Save Continuously is off by default. With it on, every finished edit of a set that has a file is committed within a second: opening the set (in Beatlayer or `bl-cli`) gives the song after it, and a session that stops leaves nothing to recover. The set file keeps its revision until a normal save. Turning the option on commits the open set's unsaved edits; a set never saved has nowhere to commit, so crash recovery covers it until its first save. (S7)
- **R9.** With Save Continuously on, the set keeps a history of save points since its last normal save, each restorable as the song it was. (S7)
- **R10.** Save works as before whatever the setting: it writes the next revision of the set file, with its backup, and retires the save points. (S1, S7)
- **R11.** The setting is a switch labelled Save Continuously on the Settings window's File & Folder page (L-SET-015), where Live keeps its file settings; its command is `settings.set_continuous_save`. (S7)

**The save history (IMP-05; WP-6.39)**

- **R12.** File ▸ Save History, straight after Open Beatlayer Set Backups (M-FILE-008), runs `view.toggle_save_history`, which shows the open set's save history in the right-hand area where the Undo History opens (L-WIN-008, L-OTH-001), and hides it again; its close button hides it too. Whether it shows is view state: not saved with the set and no undo step. The item's name and place are Beatlayer's own. (S7, S8)
- **R13.** The history lists, newest first: the set's save points (R9), each named by its edit's label as the undo history names it, with the time it was journalled; then the set as last saved, the revision its file holds, labelled Saved with the time of that save; then each of the set's backups in its project folder's `Backup` folder (S9), labelled Saved with the time in its name. Times show in local time, with seconds for save points, and with the date for any not of today. A set never saved has no history. (S7, S8, S9)
- **R14.** With Save Continuously off the history has no save points, only the saved versions, and a line above them says that Save Continuously keeps a save point after every edit, with a button that opens Settings at the File & Folder page (R11). (S7)
- **R15.** The history follows the set while it shows: each save point appears at the top as it's journalled (R1, R8), and a normal save, which retires the save points (R10), clears them from the list and puts the new save at the top, the version it replaced among the backups. (S7, S9)
- **R16.** Selecting a row and clicking Restore at the history's foot, or double-clicking the row, or pressing Return with it selected, runs `set.restore` for it: the song becomes exactly the song that row holds (a save point's saved form, the set file's song, or a backup's song, migrated as opening it would migrate it), as one undo step named `Restore Save Point` or `Restore Saved Version`, which Undo takes back. The set keeps its file, its name and its project folder, and the transport isn't stopped. The undo names are Beatlayer's own. (S7, S9)
- **R17.** A restore is an edit like any other: it's journalled (R1), and with Save Continuously on it's a new save point at the top of the history, so every row listed before stays listed and can still be restored. (S7)
- **R18.** A row whose song can't be read any more (its journal damaged, a backup moved away) restores nothing: the song stays as it was and the Status Bar says why. (S7)
- **R19.** The history is a list for assistive technology, each row reading its label and its time, and Restore a button; every part works from the keyboard. (S9)
- **R20.** `set.save_history` (read) reports the rows R13 lists, newest first, each with its `id`, its kind (`save_point`, `saved` or `backup`), its time in UTC (RFC 3339) and its label, and whether Save Continuously is on. `set.restore { "id": … }` restores one, in the `destructive` class, since it replaces the whole song and an agent asks first; it's read back through the query tool's `song` (`undo.next_undo` names the restore). `view.toggle_save_history` is in the read class, and `view.state` reports whether the history shows, as it reports the Undo History. (S7, S9)

**Crash recovery and Save Continuously in the app (WP-6.62)**

- **R21.** The app runs crash recovery from its start: before the first set opens it registers its session in the recovery folder in the user's application data, and while a set is open it journals every finished edit as R1 says, whoever makes it (a gesture, a key, a menu item, an undo or a redo, an agent or the assistant), on the journal's own thread, so neither the interface nor the audio waits on the disk. Opening, saving, Save As, New and closing tell the journal what R6 and R7 need; quitting after the person has answered the save question closes the set, so nothing is offered at the next launch. (S6, S7, S10)
- **R22.** When the app starts and R2 finds sets to offer, a dialog over the main window, once the start-up set (app-shell R1) has opened, lists them newest first, each with its name, its file or "never saved", how many edits follow its last save, the time and name of its last edit, and a warning when the set file has been saved since by something else; each has Recover and Discard buttons, and the dialog has a Later button. Recover runs `set.recover` for that set (R3), in place of the start-up set; Discard runs `set.discard_recovery` (R4); Later closes the dialog and leaves every set still listed to be offered at the next launch. The dialog closes by itself once nothing is left in it. Its wording and the Later button are Beatlayer's own. (S6, S10)
- **R23.** The dialog reaches assistive technology as a dialog naming each set, its buttons in the keyboard's Tab order, Recover on the newest set focused first; Esc does what Later does. (S10)
- **R24.** Settings ▸ File & Folder has the Save Continuously switch R11 names, off on a new install; it's kept between launches in the preferences, `settings.set_continuous_save` sets it, and `settings.state` reports it as `file_folder.save_continuously`. Turning it on commits the open set's edits (R8). (S7, S10)
- **R25.** With Save Continuously on, a set that has a file has no unsaved changes once its last edit is committed (R8): the query tool's `set.unsaved_changes` reads false, and New Set, Open Set and Quit don't ask whether to save it (app-shell R4). A set never saved still asks. This is Beatlayer's own. (S7, S10)
- **R26.** `set.list_recoverable`, `set.recover` and `set.discard_recovery` run against the app's own recovery, through the agent API as from the dialog, with their permission classes (R2 to R4). `bl-cli` sessions journal only when given a recovery folder (`--recovery-dir`), so headless runs leave nothing in the app's. (S7, S10)
- **R27.** When the journal can't be written (the disk full, the folder gone), the Status Bar says once that edits aren't being kept for recovery, the app goes on working, and each later edit tries again; the message goes when a write succeeds. The wording is Beatlayer's own. (S7)
- **R28.** A session killed at any moment while the app runs with a set open is offered at the next launch with every edit the app journalled before it was killed, recovered through the dialog to the same song as the library's recovery gives (R2, R3). (S6, S7)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/project/tests/crash_recovery.rs`, `edits_are_journalled_within_a_second` (a 200-track, 5,000-clip song) | integration |
| R2, R3 | `crates/project/tests/crash_recovery.rs`, `a_killed_session_is_offered_and_every_edit_recovered` (child processes killed at random; recovered through the registered commands); `crates/project/src/autosave.rs`, `a_session_that_stops_is_offered_and_recovers` | integration, unit |
| R4 | `crates/project/src/autosave.rs`, `discarding_keeps_a_copy_and_removes_the_edits` | unit |
| R5 | `crates/project/src/autosave.rs`, `a_session_that_closes_leaves_nothing`; `crates/project/tests/crash_recovery.rs`, `a_killed_session_is_offered_and_every_edit_recovered` (another instance while the child runs) | unit, integration |
| R6 | `crates/project/src/autosave.rs`, `a_session_that_closes_leaves_nothing`, `saving_retires_the_journal_and_continuous_save_commits` | unit |
| R7 | `crates/project/src/autosave.rs`, `beginning_another_set_keeps_the_last_ones_edits` | unit |
| R8 | `crates/project/tests/crash_recovery.rs`, `with_save_continuously_a_killed_session_reopens_at_its_last_edit`, `with_save_continuously_every_journalled_edit_is_saved`; `crates/project/src/autosave.rs`, `saving_retires_the_journal_and_continuous_save_commits` | integration, unit |
| R9 | `crates/project/src/journal.rs`, `committed_entries_are_save_points`; `crates/project/src/autosave.rs`, `saving_retires_the_journal_and_continuous_save_commits` | unit |
| R10 | `crates/project/src/autosave.rs`, `saving_retires_the_journal_and_continuous_save_commits` | unit |
| R11 | `crates/ui/tests/recovery.rs`, `the_file_and_folder_switch_turns_save_continuously_on`; `crates/project/src/registry.rs`, `every_command_is_registered_and_complete` covers the command. | UI script |
| R12 | to be written in WP-6.39 | |
| R13 | to be written in WP-6.39 | |
| R14 | to be written in WP-6.39 | |
| R15 | to be written in WP-6.39 | |
| R16 | to be written in WP-6.39 | |
| R17 | to be written in WP-6.39 | |
| R18 | to be written in WP-6.39 | |
| R19 | to be written in WP-6.39 | |
| R20 | to be written in WP-6.39 | |
| R21 | `crates/shell/tests/recovery.rs`, `a_killed_session_is_offered_and_recovered` (the shell journals the set's edits) | integration |
| R22 | `crates/ui/tests/recovery.rs`, `the_recovery_prompt_lists_the_crashed_sets_newest_first`, `recovering_from_the_prompt_opens_the_set`, `esc_is_later_and_keeps_the_set_for_the_next_launch` | UI script |
| R23 | `crates/ui/tests/recovery.rs`, `the_recovery_prompt_lists_the_crashed_sets_newest_first` (the tree), `esc_is_later_and_keeps_the_set_for_the_next_launch` (Esc) | UI script |
| R24 | `crates/ui/tests/recovery.rs`, `the_file_and_folder_switch_turns_save_continuously_on` | UI script |
| R25 | `crates/shell/tests/recovery.rs`, `save_continuously_leaves_a_set_with_a_file_clean` | integration |
| R26 | `crates/agent/tests/recovery.rs`, `an_agent_recovers_a_set_a_crash_left`; `crates/shell/tests/recovery.rs`, `discarding_a_recovery_leaves_nothing_to_offer` | integration |
| R27 | `crates/shell/tests/recovery.rs`, `a_read_only_recovery_folder_says_so_once_and_the_set_still_edits` | integration |
| R28 | `crates/shell/tests/recovery.rs`, `a_killed_session_is_offered_and_recovered` | integration |

## Beatlayer's own choices

These don't come from Live; they follow from the architecture and IMP-05.

| Choice | Test |
|---|---|
| The journal is a hidden file beside the set, `.<set file>.<session>.bljournal`, one per session, so sessions never write the same file. A set never saved journals in the recovery folder. If the project folder can't be written (read-only, or a drive gone), the journal moves to the recovery folder. | `crates/project/src/autosave.rs`, `a_read_only_project_journals_in_the_recovery_folder` |
| Each journal entry is a zstd frame, like a set file's: a snapshot of the whole saved form, or a patch from the entry before. A write cut short leaves a damaged last frame, which reading ignores. | `crates/project/src/journal.rs`, `a_cut_or_damaged_tail_is_ignored`; `crates/project/src/patch.rs`, `any_patch_round_trips` |
| Edits that arrive faster than the disk takes them are written as one entry holding the newest song, so the journal never falls behind. | `crates/project/tests/crash_recovery.rs`, `edits_are_journalled_within_a_second` |
| Sessions register in a recovery folder in the user's application data: a lock file the running session holds (the operating system lets go when the process dies, however it dies) and an index saying where its journal is. | `crates/project/src/autosave.rs`, `a_session_that_closes_leaves_nothing` |
| A journal applies to a set only if it follows that set file's revision and fingerprint, so a set replaced by another program never picks up stale save points. | `crates/project/src/journal.rs`, `committed_entries_are_save_points` |
| Sessions and journal entries from a newer version of Beatlayer are left for that version: an older build sharing the recovery folder neither offers nor tidies them. | `crates/project/src/autosave.rs`, `a_newer_versions_sessions_are_left_alone`; `crates/project/src/journal.rs`, `a_newer_versions_entries_are_recognised` |

# Crash recovery and Save Continuously

- **Status:** partial (R11, the switch in Settings, waits on the Settings window's File & Folder page; every other rule has a passing test)
- **Work packages:** WP-1.11 (the journal, crash recovery, the Save Continuously option, and their commands). The recovery prompt and the switch in the Settings window are the app shell's to draw (WP-1.7 and the Settings pages after it); the history browser for save points is Phase 6 (IMP-05).
- **Parity-matrix row:** Files and export: Crash recovery; (Beatlayer only) Save Continuously
- **Checked against:** the Live 12 reference manual (online edition) and the Live 12 release notes, read on 10 October 2026.

## The behaviour

You're working on a set and Live stops: it crashes, you force it to quit, the power goes. The next time you start Live, it offers to bring the set back as it was when it stopped, including the edits you hadn't saved. Say yes and you carry on; say no and Live starts afresh, keeping the recovery files aside for a while in case you change your mind.

Beatlayer does the same, always: while a set is open, every finished edit is written to a journal beside the set within a second, and a set that was open when Beatlayer stopped without closing it is offered at the next launch, exactly as it was after its last journalled edit. Beatlayer adds one option Live doesn't have (IMP-05): **Save Continuously**, off by default. With it on, every finished edit of a set that has a file is saved within a second, so reopening the set shows it without any question, and the set keeps a history of save points since its last normal save. Save (Cmd-S) works as it always does.

This spec covers when edits reach the disk, when a set is offered, what recovering and declining do, what several running instances do, and the Save Continuously option. How the prompt looks is the app shell's, and the history browser is Phase 6's.

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
| R11 | None yet: the File & Folder page isn't built. `crates/project/src/registry.rs`, `every_command_is_registered_and_complete` covers the command. | |

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

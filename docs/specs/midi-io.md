# MIDI I/O: ports, track inputs, monitoring, timestamps, and the computer MIDI keyboard

- **Status:** draft
- **Work packages:** WP-2.2
- **Parity-matrix row:** Control and mapping: Computer MIDI keyboard; Recording: Monitoring In, Auto, Off (the monitoring switch only; recording is WP-2.13 and WP-2.14)
- **Checked against:** the Live 12 reference manual as the UI parity reference cites it (read 10 October 2026), and the parity reference's observations of Live 12.4.3 (10 October 2026).

## The behaviour

Live lists every MIDI port the computer offers in its Settings, inputs and outputs separately, and gives each port three switches that decide what it's for: Track (its notes and controllers reach tracks), Sync (MIDI clock and transport), and Remote (controlling Live's parameters through MIDI mapping). A MIDI track takes its input from all inputs whose Track switch is on, from the computer MIDI keyboard, from one port, or from nothing, on all channels or on one; its monitoring switch (In, Auto, Off) says when what arrives is heard. The computer MIDI keyboard turns the letter keys into a small keyboard, switched on and off with M, with keys to move its octave and change its velocity; while it's on, the letters it plays don't run their shortcuts.

Beatlayer keeps the port switches in its preferences and the track's input and monitoring in the set. Messages arrive on the MIDI system's own thread, are stamped with the time they arrived, and cross to the audio thread on a lock-free queue; the engine places each one in the next block at the sample its time gives, and hands it to every MIDI track whose input it matches.

This spec covers ports and their switches, a MIDI track's input and channel, the monitoring switch, how incoming messages reach the engine, and the computer MIDI keyboard. It leaves to other packages: what a track's devices do with the notes (WP-2.1), recording them (WP-2.14), an audio track's input and audio monitoring (WP-2.13), routing MIDI from one track to another (WP-2.19), MIDI output from tracks, MIDI clock and MIDI mapping (Phase 7, M7), control surfaces and Takeover Mode (Phase 7), and MPE (the MPE column and MPE input, with the packages that play MPE).

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 2 "First Steps", section 2.3.5, the Link, Tempo & MIDI settings | The MIDI Ports list names every input and output; each has Track, Sync, Remote, and MPE switches. Track on an input lets its messages reach tracks; Sync on an input lets Live follow its clock; Remote on an input lets it control Live through MIDI mapping. On an output they let tracks send to it, send clock, and send feedback for mapped controls. |
| S2 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in [layout.md](../ui-parity/layout.md) L-SET-002 and L-SET-014 (WP-0.13) | The Tempo & MIDI settings: the Tempo Follower section, then MIDI: the Control Surfaces table, Takeover Mode, then the MIDI ports table with its switch columns. The page chooser lists Link before Tempo & MIDI. |
| S3 | Manual | Live 12 reference manual, chapter 17 "Routing and I/O", the track input choosers, as [layout.md](../ui-parity/layout.md) L-MIX-001 and L-ARR-008 cite it | A MIDI track's input type chooser offers All Ins, Computer Keyboard, each input port, other tracks, and No Input; the channel chooser offers All Channels and channels 1 to 16. |
| S4 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in layout.md L-MIX-001 and L-ARR-008 | The In/Out section shows the input type, the input channel, and the Monitor switch with In, Auto, and Off, in the Session mixer and in the Arrangement's track header column. |
| S5 | Manual | Live 12 reference manual, chapter 17 "Routing and I/O", monitoring | In: the track always monitors its input, armed or not. Auto: it monitors while armed. Off: it never monitors. |
| S6 | Manual | Live 12 reference manual, section 42.24 (computer keyboard and map mode keys), as recorded in [shortcuts.toml](../ui-parity/shortcuts.toml) S-MAP-003 to S-MAP-007 | M switches the computer MIDI keyboard on and off; Z and X move its octave down and up; C and V lower and raise its velocity; the home-row letters play white keys and the row above plays black keys (shown as a figure); a single-letter shortcut is reached with Shift while the keyboard is on. |
| S7 | Observation | Live 12.4.3 on macOS, 10 October 2026, recorded in layout.md L-CTRL-020 and L-CTRL-026 and [menus.md](../ui-parity/menus.md) M-OPT-004 | The Control Bar's Computer MIDI Keyboard switch, and its MIDI track input and output indicators; the Options menu's Computer MIDI Keyboard item, showing M. |
| S8 | Paper | Fons Adriaensen, "Using a DLL to filter time", Linux Audio Conference 2005 | A second-order delay-locked loop that turns noisy callback times into a smooth estimate of when each audio block starts. |

## Rules

### Ports and their switches

- **R1.** Beatlayer lists every MIDI input and output port the platform's MIDI system offers (Core MIDI on macOS, the ALSA sequencer on Linux, Windows Multimedia MIDI on Windows), each by its name, inputs and outputs separately. (S1, S2)
- **R2.** Each port has a Track, a Sync, and a Remote switch, in that order, on the Tempo & MIDI page of the Settings window, after the page's other MIDI settings (L-SET-014). Its row reads "In: <name>" or "Out: <name>". (S1, S2)
- **R3.** The switches are kept in the preferences by direction and port name, so they apply again whenever the port is seen, after a restart or a replug. A port seen for the first time starts with Track on for an input and every switch off for an output. (S1; the starting state is Beatlayer's choice)
- **R4.** A port is open while any of its switches is on, and closed when all three are off. A port that can't be opened keeps its switches and reports why. (Beatlayer's choice)
- **R5.** An input's messages reach tracks only while its Track switch is on. Messages from an input with Remote on go to remote control, and its clock and transport messages, with Sync on, go to sync; both wait for Phase 7, which maps and follows them, so for now they're only counted. A message from an input whose Track switch is off never reaches a track. (S1)
- **R6.** Listing the ports is a read-class command (`midi.ports`). Changing a switch is an external-class command (`midi.set_port`), because it opens or closes a device outside Beatlayer. (D17)

### A MIDI track's input and monitoring

- **R7.** A MIDI track's input is the In/Out section's MIDI From chooser, as [routing](routing.md) R1, R2, and R16 give it: All Ins (every input whose Track switch is on, and the computer MIDI keyboard), Computer Keyboard, one input port by name, another MIDI track, or No Input, with All Channels or one channel from 1 to 16 (a channel belongs to a port input). The channel chooser lists All Channels and 1 to 16 only with a port input; with All Ins or Computer Keyboard it shows All Channels and can't be changed. A new MIDI track takes All Ins on All Channels, and the input and its channel live in the track's routing, saved and changed as routing.md R8 and R20 say. (S3)
- **R8.** A track whose input names a port receives that port's messages while the port is present and its Track switch is on. The track keeps the name when the port goes away, and receives again when a port of that name returns. (S3; Beatlayer's choice for absent ports)
- **R9.** With one channel chosen, only channel messages on that channel reach the track. System messages (clock, start, stop, song position, system exclusive) never reach a track's input. (S3)
- **R10.** Audio and MIDI tracks have monitoring: In, Auto, or Off, Auto on a new track. In: the track monitors its input whether or not it's armed. Auto: it monitors while armed. Off: it never monitors. Group, return, and Main tracks have neither an input nor monitoring, and commands that would give them one are refused. (S4, S5)
- **R11.** A track's input, its channel, and its monitoring belong to the set, as [routing](routing.md) R8 and R20 give them: they're saved with it, a set saved before they existed loads with the defaults, and each change is one undo step. They change through the routing commands `routing.set_input` and `routing.set_monitor`, edit-class commands. (S4; D17)
- **R12.** Every message that matches a MIDI track's input reaches that track's MIDI input in the engine, monitored or not, and the engine says in each block whether the track monitors (R10), so recording takes what an armed track receives ([recording](recording.md) R6, WP-2.14). While the track monitors, its devices play what its input received: each note-on and note-off with its velocity and release velocity, and each key's pressure as a per-note expression, reach the first device in the chain on the frames R15 gives them, merged in frame order with the notes of the track's clips, a note-off before a note-on on one frame ([device chains](device-chains.md)). A track that doesn't monitor sends its devices nothing from its input. Channel-wide messages (control changes, pitch bend, channel pressure) reach the first device in the chain the same way, in frame order with the notes ([device framework](device-framework.md) R39). A block with more notes than its note buffer holds drops the rest and counts them, as R14 does. (Beatlayer's design)

### From the port to the engine

- **R13.** A message is stamped on the MIDI system's thread with the time it arrived, on the same monotonic clock the engine reads. Where the platform stamps messages itself (Core MIDI, ALSA), its stamps are mapped onto that clock, which keeps the spacing of messages that arrive together. In tests the clock is a test clock, which the null device's manual pacing advances by each block's length, so a message a test plays a musical time after another lands on that frame however busy the machine is. (Beatlayer's design)
- **R14.** Messages cross from the MIDI threads (and the computer keyboard) to the audio thread on a lock-free queue. The audio thread takes them without allocating, locking, or blocking, and a sender never makes it wait. When the queue or a block is full, what doesn't fit is dropped and counted. (The architecture's real-time rules)
- **R15.** The engine places a message in the block after the one during which it arrived, at the sample offset its time gives, counted from when the previous block started. Messages therefore keep their spacing to the sample, at a fixed latency of one block. A message stamped before the previous block started lands on the block's first frame and is counted as late; one stamped after the current block started waits for the next block. (Beatlayer's design)
- **R16.** The engine estimates when each block starts with a delay-locked loop over the audio callback's times and its frame count, so jitter in when the callback runs doesn't move the messages it places. It locks again after a gap, such as the device stopping. Under the null device's manual pacing the callback times are the test clock's, which advances exactly one block a block, so the loop sees no jitter and no gap. (S8)

### The computer MIDI keyboard

- **R17.** M switches the computer MIDI keyboard on and off, as do the Control Bar's Computer MIDI Keyboard switch (L-CTRL-020) and the Options menu's item (M-OPT-004). Whether it's on is kept between starts. (S6, S7; keeping it is Beatlayer's choice)
- **R18.** While it's on, the home row plays white keys and the row above black keys, from A: A C, W C♯, S D, E D♯, D E, F F, T F♯, G G, Y G♯, H A, U A♯, J B, K C, O C♯, L D, P D♯. (S6; the key list is S-MAP-006's)
- **R19.** Z and X move the keys down and up an octave. A plays C3 (MIDI note 60, in Live's octave numbering) at start, and can go from C-2 (note 0) to C8 (note 120); a key whose note would pass 127 plays nothing. (S6)
- **R20.** C and V lower and raise the velocity through 1, 20, 40, 60, 80, 100, 120, and 127, starting at 100. (S6)
- **R21.** A key plays its note while held: a note-on at the current velocity when pressed, a note-off when released, on MIDI channel 1. Holding it doesn't repeat the note. The note-off goes to the note the key started, even if the octave moved while it was held, and switching the keyboard off releases every note it holds. (S6)
- **R22.** While it's on, the letters it uses (its note keys, Z, X, C, and V) don't run their single-letter shortcuts; with Shift, they do. Other letters, M among them, keep their shortcuts, as do keys pressed with Cmd, Ctrl, or Option (Alt). While a text field has the keyboard, every key types. (S6)
- **R23.** Its notes reach the tracks whose input is Computer Keyboard or All Ins, as a source of their own. (S3)
- **R24.** Its actions are commands people and agents run by ID: `midi.toggle_computer_keyboard` (M; edit-class, as a kept preference), and `midi.computer_keyboard_octave`, `midi.computer_keyboard_velocity`, and `midi.computer_keyboard_key` (read-class: they change nothing in the set or on disk). (D17)
- **R25.** The Control Bar's MIDI track input indicator (L-CTRL-026) lights for a moment whenever a message reaches the tracks' inputs. The lower lamp is the output indicator: it lights the same way whenever a message leaves for a port ([external devices](external-devices.md) R7). (S7)
- **R26.** No note hangs: every note-on a track's devices received from its input gets a note-off on the first frame of the block after the edit that ends it, when monitoring stops (Monitor Off, or Auto with the track disarmed), or the track's input or channel changes. A track that is deleted takes its devices with it. Edits that change none of these keep the notes sounding. (Beatlayer's design)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R3, R17 | `bl-shell` `midi_io::port_switches_and_the_keyboard_persist_across_restarts` | integration |
| R2 | `bl-ui` `midi_input::the_tempo_and_midi_page_lists_each_port_with_its_switches` | UI |
| R4, R5 | `bl-shell` `midi_io::switching_a_ports_track_off_stops_its_notes_reaching_tracks` | integration |
| R7, R8, R9 | `bl-shell` `midi_io::midi_from_one_port_ignores_another_and_a_channel_ignores_the_rest`, `midi_io::midi_from_lists_the_ports_and_the_channel_chooser_follows_the_input` | integration |
| R10, R12 | `bl-engine` `midi_play::a_monitored_tracks_devices_play_its_input_on_the_exact_frames`, `midi_play::input_merges_with_a_playing_clips_notes_in_frame_order`, `midi_play::a_track_that_doesnt_monitor_sends_its_devices_nothing`; `bl-shell` `midi_io::the_computer_keyboard_plays_snapshot_on_a_monitoring_track`; `rtsan` `midi_input` | integration, real-time |
| R26 | `bl-engine` `midi_play::switching_monitoring_off_ends_the_held_notes`, `midi_play::disarming_a_track_on_auto_ends_the_held_notes`, `midi_play::changing_the_input_or_its_channel_ends_the_held_notes`, `midi_play::an_edit_that_keeps_the_input_keeps_the_held_notes_sounding` | integration |
| R14 | `bl-shell` `midi_io::the_audio_hosts_engine_takes_the_queue`; `bl-midi-io` `queue::tests::attaching_again_sends_to_the_new_end_only` | integration, unit |
| R17 | `bl-ui` `midi_input::the_control_bars_switch_follows_and_changes_the_keyboard` | UI |
| R18 to R21 | `bl-shell` `midi_io::the_keyboards_keys_play_the_right_pitches_and_octave_keys_shift_by_twelve`, `midi_io::a_note_ends_the_note_it_started_and_switching_the_keyboard_off_ends_every_note`; `bl-ui` `midi_input::a_note_ends_when_the_window_loses_the_keyboard` | integration, UI |
| R22 | `bl-ui` `midi_input::m_switches_the_keyboard_on_and_its_letters_play_notes` | UI |
| R23 | `bl-shell` `midi_io::the_computer_keyboard_reaches_tracks_set_to_it_or_all_ins_only` | integration |
| R6, R24 | `bl-cli` `agent_midi_io` (each `midi.*` command by ID through `bl-cli mcp`; `midi.set_port` refused without `--autonomous`) | agent API |

# MIDI Timecode: following incoming timecode as a position in the Arrangement

- **Status:** draft
- **Work packages:** WP-7.13 (R1 to R12). The ports and EXT are [MIDI clock](midi-clock.md)'s (R14, R15); the engine's transport is [transport](transport.md)'s
- **Parity-matrix row:** Sync and control: MIDI Timecode in
- **Checked against:** the Live 12 reference manual (online edition), read 23 October 2026 (chapter 17 sections 17.3.1.2 and 17.3.4, chapter 37 sections 37.3, 37.3.2, 37.3.2.1 and 37.3.3); the MIDI 1.0 specification's MIDI Time Code; the UI parity reference rows L-SET-014, M-OPT-007 and M-OPT-010

## The behaviour

MIDI Timecode is the MIDI form of SMPTE timecode, which video and tape machines run on: a time in hours, minutes, seconds and frames. A timecode host sends it as quarter-frame messages while it runs, eight of which spell out one whole time over two frames, and as a single full-frame message when it jumps. Beatlayer only follows timecode; it never sends it.

An input port with Sync on can be set to follow timecode instead of clock. Its options then add a frame rate (one of the usual rates, or SMPTE All to take whatever rate the messages carry) and an offset, the timecode at which the Arrangement starts. With EXT on, each incoming time becomes a position in the Arrangement: the time less the offset, from 1.1.1, through the set's tempo map. Timecode says nothing about tempo or bars, so Beatlayer keeps playing at its own tempo; it starts when the timecode runs, stops when it stops, and moves when it jumps.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 37 "Synchronizing with Link, Tempo Follower, and MIDI", section 37.3 | MIDI Timecode is SMPTE over MIDI: a time in seconds and frames. Live reads it as a position in the Arrangement; it carries no tempo or meter, so tempo is set by hand and tempo changes aren't tracked. Live can only follow timecode, not send it |
| S2 | Manual | The same chapter, section 37.3.2.1 "MIDI Timecode Options" | Timecode options are set per port, from the ports list. The MIDI Timecode Frame Rate applies only with MIDI Timecode chosen as the port's MIDI Sync Type, and offers the usual SMPTE rates and SMPTE All, which detects the rate of the incoming messages; the Arrangement's own display format is separate (Options ▸ Time Ruler Format). The MIDI Timecode Offset, an SMPTE time, is read as the Arrangement's start |
| S3 | Manual | The same chapter, section 37.3.3 "Sync Delay" | A Sync Delay per port delays Live's time base against the sync signal |
| S4 | Manual | Chapter 17 "Routing and I/O", sections 17.3.1.2 "Sync" and 17.3.4 "MIDI In/Out Indicators" | Sync on an input lets Live receive MIDI clock or timecode; the first pair of Control Bar lamps is for clock and timecode used for sync |
| S5 | Standard | The MIDI 1.0 specification: MIDI Time Code | A quarter-frame message (F1, one data byte) carries one of eight pieces (frames low and high, seconds low and high, minutes low and high, hours low and high with the rate), sent four per frame, so a whole time takes two frames and describes the time when its first piece was sent. A full-frame message is a universal real-time system exclusive message (F0 7F, a device ID, 01 01, hours with the rate, minutes, seconds, frames, F7) sent when the time jumps. The rate code is 24, 25, 30 drop-frame (29.97 frames a second) or 30 frames a second |
| S6 | Reference | [layout.md](../ui-parity/layout.md) L-SET-014 (the MIDI ports table); [menus.md](../ui-parity/menus.md) M-OPT-007 (External Sync) and M-OPT-010 (Time Ruler Format: Time, 24 fps, 25 fps, 30 fps, 29.97 fps drop frame and non-drop) | Where the options and the switch are; the rates the Arrangement displays |
| S7 | Spec | [MIDI clock](midi-clock.md) R1, R2, R14 to R16, R20, R25; [MIDI I/O](midi-io.md) R5, R9, R13 to R15; [transport](transport.md) R3, R7, R15 | The sync options, EXT and its lamps, arrival stamps and placement; stopping and locating on exact frames through the tempo map |

## Rules

- **R1.** An input port's sync options ([MIDI clock](midi-clock.md) R14) start with MIDI Sync Type, a chooser of MIDI Clock and MIDI Timecode (MIDI Clock for a port seen for the first time). With MIDI Timecode chosen, two more rows show: MIDI Timecode Frame Rate, a chooser of 24 fps, 25 fps, 29.97 fps (drop frame), 30 fps and SMPTE All (SMPTE All at first), and MIDI Timecode Offset, an hours:minutes:seconds:frames field (00:00:00:00 at first). They're kept in the preferences with the port, set through `midi.set_port` (`sync_type`, `timecode_rate`, `timecode_offset`) and read back through `midi.ports`. (S2, S6; the starting values and the list's order are Beatlayer's own)
- **R2.** From an input with Sync on and MIDI Timecode chosen, quarter-frame and full-frame messages go to sync, never to tracks, stamped with their arrival time; a full-frame message is read as the system exclusive message it is, before anything is refused as too long for a track. Clock messages from that port are ignored, and timecode from a port set to MIDI Clock is ignored. (S4, S5, S7)
- **R3.** With a fixed rate chosen, every time is counted at that rate, whatever rate the messages carry; with SMPTE All, each time at the rate its own hours byte names. A time becomes seconds as hours, minutes and seconds plus its frames over the rate; at 29.97 fps drop frame, frame numbers 0 and 1 are skipped at the start of every minute except every tenth, and the second has 30,000/1,001 frames. (S2, S5)
- **R4.** A time T is the Arrangement time T less the MIDI Timecode Offset, counted from 1.1.1, and becomes a song position through the set's tempo map ([transport](transport.md) R15); a time before the offset is 1.1.1. Beatlayer's tempo stays its own: the tempo map plays as it does without sync, automation included. (S1, S2)
- **R5.** With EXT on and timecode leading ([MIDI clock](midi-clock.md) R17's choice among inputs), a full-frame message while stopped locates there, as a locate does ([transport](transport.md) R7). (S1, S5)
- **R6.** Quarter frames running forward start playback: once a whole time has arrived in order (eight pieces), Beatlayer plays from that time plus the two frames it took to send, on the frame the last piece's arrival gives ([MIDI clock](midi-clock.md) R20's placement and Sync Delay). (S1, S5; starting on the first whole time is Beatlayer's own)
- **R7.** While timecode runs and Beatlayer plays, each whole time is compared with Beatlayer's own position at the moment it describes; when they differ by more than one frame, Beatlayer jumps to the timecode's position, as a locate while playing does. Smaller differences are left alone: Beatlayer plays at its own tempo between them. (S1; the one-frame tolerance is Beatlayer's own)
- **R8.** When no quarter frame has arrived for 100 ms, or pieces arrive in reverse order, the timecode has stopped: Beatlayer stops, as Stop while playing does ([transport](transport.md) R3). A full-frame message while playing jumps there. (S5; the 100 ms is Beatlayer's own)
- **R9.** While timecode leads, the Arrangement loop doesn't wrap playback: the timecode's position is played as it is. (Beatlayer's own)
- **R10.** The upper sync lamp ([MIDI clock](midi-clock.md) R16) lights for 100 ms each time a whole time arrives from a timecode input, whether EXT is on or not. (S4)
- **R11.** Beatlayer never sends timecode: an output's sync options ([MIDI clock](midi-clock.md) R1) have no timecode rows. (S1)
- **R12.** `sync.state` reports, for a timecode input, the last whole time with its rate, whether it runs, and the position it gives; reading pieces, assembling times and following them run on the audio thread without allocating, locking or blocking, the full-frame message having been parsed into a fixed-size message on the MIDI system's thread. (S7)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-7.13 | shell test, UI script |
| R2 | to be written in WP-7.13 | `bl-midi-io` test |
| R3, R4 | to be written in WP-7.13 | unit (every rate, drop-frame minutes, an offset, a tempo map with a change) |
| R5, R6, R7, R8, R9 | to be written in WP-7.13 | engine test under the allocation trap (timecode from a test port at test-clock times) |
| R10 | to be written in WP-7.13 | shell test |
| R11 | to be written in WP-7.13 | shell test |
| R12 | to be written in WP-7.13 | agent API, real-time trap |

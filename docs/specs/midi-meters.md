# MIDI meters: MIDI activity on MIDI tracks and between devices

- **Status:** implemented (every rule below has a passing test)
- **Work packages:** WP-6.69 (R1 to R8). The track's level meter itself is [mixer](mixer.md) R19 and [the Mixer view](mixer-view.md) R10; the device chain's layout is [device chains](device-chains.md) R22; [MPE](mpe.md) R33, the blue dot for per-note expression, is R5 here
- **Parity-matrix row:** Mixing and routing: Track meters, MIDI activity on MIDI tracks, and the meters between devices
- **Checked against:** the Live 12 reference manual (online edition), read 20 October 2026 (chapter 3 section 3.14, chapter 12 section 12.1, chapter 17 sections 17.3 and 17.3.4, chapter 18 section 18.1, chapter 23 section 23.2); the UI parity reference rows L-DEV-003, L-MIX-003, L-ARR-006 and L-CTRL-026

## The behaviour

A MIDI track carries MIDI until its instrument turns it into audio. Where a meter would show an audio level and the signal there is MIDI, Beatlayer shows a MIDI meter instead: a column of dots that lights to the velocity of each note passing it and falls back, its lowest dot lighting for controllers, pitch bend and pressure, and lighting blue for a note's own per-note expression. A MIDI track with no instrument and no audio effect shows a MIDI meter as its track meter, in the Mixer and in the Arrangement's track header, for the MIDI leaving its chain. In the Device View, the meters between devices show what passes from one device to the next: MIDI meters before the instrument, level meters after it.

The meters only report what plays. The engine publishes what passed each point in each block, the window draws it, and agents read the same values without the window.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 23 "Working with Instruments and Effects", section 23.2 "Using Devices" | Devices in a chain have input and output level meters, which show where a signal goes missing; on a MIDI track the signals before the instrument are MIDI and those after it audio |
| S2 | Manual | Chapter 3 "Live Concepts", section 3.14 "The Mixer" | A MIDI track with no instrument and no audio effects puts out MIDI, which has to go elsewhere to be heard |
| S3 | Manual | Chapter 18 "Mixing", section 18.1 | A track's meter shows its peak and RMS output levels, or its input's while it monitors |
| S4 | Manual | Chapter 12 "Editing MPE", section 12.1 "Viewing MPE Data" | A MIDI track's meter lights its lowest dot blue while per-note controller changes pass it |
| S5 | Manual | Chapter 17 "Routing and I/O", sections 17.3 and 17.3.4 | MIDI activity shows beside each entry of the MIDI input channel chooser and on the Control Bar's MIDI indicators |
| S6 | Beatlayer | The UI parity reference: [layout.md](../ui-parity/layout.md) L-DEV-003 (devices left to right with level meters between them), L-MIX-003 (the strip's meter), L-ARR-006 (the track header's meter column), L-CTRL-026 (the Control Bar's MIDI indicators) | Where the meters sit |
| S7 | Beatlayer | [Mixer](mixer.md) R19; [the Mixer view](mixer-view.md) R10 and R15; [device chains](device-chains.md) R22; [device framework](device-framework.md) R30, R33 and R39; [MIDI I/O](midi-io.md) R25; the architecture's "Real-time rules" | Track meters and how they redraw; the chain's layout; notes, per-note expressions and channel messages in a block; the Control Bar's indicators; no allocation, lock or blocking call on the audio thread |

## Rules

- **R1.** A MIDI track whose chain holds no instrument and no audio effect shows a MIDI meter as its track meter, in its Mixer strip (L-MIX-003) and its Arrangement header's meter column (L-ARR-006), for the MIDI leaving the end of its chain. A MIDI track with an instrument or an audio effect keeps its level meter ([mixer](mixer.md) R19). Adding or removing the instrument switches the meter at once. (S2, S3, S6)
- **R2.** In the Device View, a meter sits before a chain's first device, between each two devices and after its last, in the gaps L-DEV-003 puts between them, showing what passes that point: a MIDI meter where the signal is MIDI (before a MIDI track's instrument, and along the whole chain of a MIDI track with none) and a level meter, showing the peak level of each channel as the track's meter does, where it's audio. A rack's shown chain has its meters the same way. (S1, S6)
- **R3.** A note-on passing a MIDI meter's point lights the meter at once to its velocity's share of the meter's height, and the meter falls back to dark over 300 ms unless a louder note lights it higher (the fall is Beatlayer's own). Note-offs light nothing. The engine does the fall, counting frames, because the window reads only the newest block and would miss a note that played and fell between two of its frames. (S1; Beatlayer's own)
- **R4.** A control change, a pitch bend or a channel pressure passing a MIDI meter's point lights its lowest dot for 300 ms (Beatlayer's own). (S1, S7)
- **R5.** A per-note expression (pitch, slide or pressure, [MPE](mpe.md) R5) passing a MIDI meter's point lights its lowest dot blue for 300 ms, over R4's colour ([MPE](mpe.md) R33). (S4; the time is Beatlayer's own)
- **R6.** After each block the engine publishes, for each MIDI meter's point, the highest note-on velocity that passed it, whether a channel message passed and whether a per-note expression passed, each as it has fallen since (R3 to R5), and for each level meter's point between devices its peak level per channel, falling as a track's meter does ([mixer](mixer.md) R19). `mixer_view.state` reports each track's meter, its kind and what it shows, and `device_view.state` each chain meter's, so agents read them headless. (S7)
- **R7.** Measuring and publishing allocates nothing, takes no lock and doesn't block on the audio thread: every meter point's slot is made with the program, and what is published depends only on what passed and the frames since, whatever the block size. While any meter is lit the window redraws each frame, and it stops once every meter is dark ([the Mixer view](mixer-view.md) R10). (S7)
- **R8.** A MIDI meter in a strip or a header is in the accessibility tree as a level indicator named MIDI Activity, its value the velocity it shows (the name is Beatlayer's own), beside the Peak Level readout's place ([the Mixer view](mixer-view.md) R15). (S7)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | `crates/ui/tests/midi_meters.rs`, `a_midi_track_with_no_devices_has_a_midi_meter_in_its_strip_and_header_that_falls_dark`, `an_instrument_switches_the_tracks_meter_to_a_level_meter_at_once`; `crates/engine/tests/midi_meters.rs`, `a_midi_track_with_no_instrument_has_a_midi_meter_at_its_end_until_one_is_added` | UI script |
| R2 | `crates/ui/tests/midi_meters.rs`, `the_device_view_draws_a_meter_in_every_gap_midi_before_the_instrument_and_audio_after_it`; `crates/engine/tests/midi_meters.rs`, `an_instrument_racks_chains_have_their_own_meters`, `an_audio_tracks_chain_has_level_meters_only` | UI script |
| R3 | `crates/engine/tests/midi_meters.rs`, `a_lit_meter_falls_to_dark_over_300_ms_and_note_offs_light_nothing`; `crates/ui/tests/midi_meters.rs`, `the_dots_light_to_the_velocity_and_the_lowest_dot_to_a_channel_message_or_an_expression`, `a_midi_track_with_no_devices_has_a_midi_meter_in_its_strip_and_header_that_falls_dark` | unit, UI script |
| R4 | `crates/engine/tests/midi_meters.rs`, `the_points_before_the_instrument_publish_what_passed_and_those_after_it_the_audio`, `a_lit_meter_falls_to_dark_over_300_ms_and_note_offs_light_nothing` | unit |
| R5 | `crates/engine/tests/midi_meters.rs`, `the_points_before_the_instrument_publish_what_passed_and_those_after_it_the_audio`; `crates/ui/tests/midi_meters.rs`, `the_dots_light_to_the_velocity_and_the_lowest_dot_to_a_channel_message_or_an_expression` | unit, UI script |
| R6 | `crates/engine/tests/midi_meters.rs`, `the_points_before_the_instrument_publish_what_passed_and_those_after_it_the_audio` (blocks of 1, 64 and 512); `crates/ui/tests/midi_meters.rs`, `an_instrument_switches_the_tracks_meter_to_a_level_meter_at_once`, `the_device_view_draws_a_meter_in_every_gap_midi_before_the_instrument_and_audio_after_it` (`mixer_view.state` and `device_view.state` by ID) | real-time trap, agent |
| R7 | `crates/engine/tests/midi_meters.rs` (every block under the allocation trap, at blocks of 1, 64 and 512); `rtsan/tests/mixer.rs`, `the_mixer_passes_under_rtsan`; `crates/ui/tests/midi_meters.rs`, `the_dots_light_to_the_velocity_and_the_lowest_dot_to_a_channel_message_or_an_expression` (redraws while lit, stops when dark) | real-time trap, RealtimeSanitizer |
| R8 | `crates/ui/tests/midi_meters.rs`, `a_midi_track_with_no_devices_has_a_midi_meter_in_its_strip_and_header_that_falls_dark`, `the_dots_light_to_the_velocity_and_the_lowest_dot_to_a_channel_message_or_an_expression` | UI script |

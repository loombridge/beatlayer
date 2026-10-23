# Link Audio: tracks streamed to and from other apps in a Link session

- **Status:** draft
- **Work packages:** WP-7.16 (R1 to R14). Link itself is [Link](link.md)'s; a peer's channel as an audio track's input is [routing](routing.md) R37
- **Parity-matrix row:** Sync and control: Link Audio
- **Checked against:** the Live 12 reference manual (online edition), read 23 October 2026 (chapter 2 section 2.3.4, chapter 17 sections 17.1, 17.2 and 17.2.1, chapter 37 sections 37.1, 37.1.1 and 37.1.3); the Link 4.1.1 public repository's Link Audio interface; the UI parity reference rows L-SET-013, L-MIX-001 and L-ARR-008

## The behaviour

Link Audio streams audio between the apps in a Link session, with no cables and no latency to set by hand. With Link and Link Audio on, every Beatlayer track that has an audio output is offered to the session as a channel, and the channels other peers offer appear in an audio track's input choosers: the peer as the Input Type, its channel as the Input Channel. Such a track monitors and records the peer's audio as it would an audio interface's input.

Audio travels with its place on the session's beat, so the receiving side plays it on the beat it was sent on, a set latency later to ride out the network. The Link page sets that latency, the name other peers see, and Sync to Incoming Audio, which delays Beatlayer's own playback by the latency so that monitored audio from peers lines up with Beatlayer's clips; its peers list shows each Link Audio peer with its state and how much of its audio is buffered.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 37 "Synchronizing with Link, Tempo Follower, and MIDI", section 37.1 | Link Audio extends Link by streaming audio in real time between the session's peers, with no extra hardware or routing |
| S2 | Manual | The same chapter, section 37.1.1 | Link Audio's settings are on the Link page: Audio switches Link Audio on and off and works only with the Link toggle on; Name is the name other peers see; Latency sets the input latency for incoming audio, and the buffered audio of each peer in the list below updates with it; Sync to Incoming Audio keeps audio received and monitored through Live in sync with Live's own audio and clips by delaying Live's playback by the latency, and only one of the session's participants should have it on; the peers list shows each connected Link Audio peer with its state and buffered audio. The Link toggle's menu lists the connected peers with Link Audio on |
| S3 | Manual | The same chapter, section 37.1.3 | With Link and Link Audio on for every peer, other participants' audio is an input source: on an audio track with monitoring In, or armed with In or Auto, the peer is chosen in the Input Type chooser and its source track in the Input Channel chooser; armed, the track records it. Every track with audio output can be streamed to peers; Live sends and receives, while other apps may only send |
| S4 | Manual | Chapter 2 "First Steps", section 2.3.4 "Link" | Link Audio streams audio between compatible devices over the network, with no manual latency compensation |
| S5 | Manual | Chapter 17 "Routing and I/O", sections 17.1 "Monitoring", 17.2 "External Audio In/Out" and 17.2.1 "Mono/Stereo Conversions" | An input is chosen with the Input Type and Input Channel choosers; monitoring In, Auto and Off; a mono input records mono and is played as identical left and right |
| S6 | Specification | The Link 4.1.1 public repository: the Link Audio part of its C interface | A peer offers named channels; each channel's description carries its own and its peer's identifier and name. A sending channel takes buffers of 16-bit samples with their channel count, frame count, sample rate and the session beat at the buffer's start, written and committed from the audio thread through real-time-safe calls. A receiving channel delivers buffers on a thread of Link's own, with the beat at their start mapped onto the local session; their memory lasts only for the call. A peer's name is set and read |
| S7 | Reference | [layout.md](../ui-parity/layout.md) L-SET-013 (the Link page), L-MIX-001 and L-ARR-008 (the In/Out section's Input Type and Input Channel choosers) | Where the settings and the choosers are |
| S8 | Spec | [Link](link.md) R1, R6, R12, R19 and R20; [routing](routing.md) R2, R8, R11, R16 and R37; [MIDI I/O](midi-io.md) R8 and R10; [recording](recording.md) R1; the architecture's "Real-time rules" | Link on and its phase; the Link page and menu; inputs, monitoring and saving routing; a missing input kept by name; recording what an armed track receives; nothing allocated, locked or blocked on the audio thread |

## Rules

- **R1.** Link Audio is off on a new install. Settings ▸ Link's Audio switch, the Link menu's Link Audio item ([Link](link.md) R19) and `link.set_audio` switch it, and whether it's on is kept in the preferences. It works only while Link is on: with Link off, nothing is sent or received and the switch keeps its state. (S2)
- **R2.** The Link page's Link Audio section (L-SET-013) holds, after the Link section ([Link](link.md) R20): Audio (a switch), Name (a text field), Latency (a slider in milliseconds), Sync to Incoming Audio (a switch) and the peers list. (S2, S7; the controls' kinds are Beatlayer's own but the slider's)
- **R3.** Name is the name other peers see Beatlayer by, kept in the preferences and set by `link.set_audio_name`; on a new install it's the computer's name. (S2; the first name is Beatlayer's own)
- **R4.** While Link Audio is on, each track with an audio output (audio tracks, MIDI tracks that hold an instrument, group tracks and return tracks) is offered to the session as a channel named as the track is, and renamed when the track is; a track added or deleted adds or removes its channel. Each block, the track's output as it leaves its mixer goes out in stereo at Beatlayer's sample rate, with the session beat of its first frame. Main isn't offered. (S3, S6; which tracks, the point taken and leaving Main out are Beatlayer's own)
- **R5.** A peer's channel is an audio track's input ([routing](routing.md) R37): the track hears it while it monitors, with In, or armed with Auto ([routing](routing.md) R11), and records it while armed and recording, as Ext. In's input is ([recording](recording.md) R1). A mono channel is played as identical left and right, and records mono. (S3, S5)
- **R6.** Incoming audio plays on the local beat it was sent on, the Latency later: each buffer's start beat is mapped onto Beatlayer's session, and its frames are placed where that beat falls, plus the Latency. Audio that arrives too late for its place is dropped and counted, and the gap is silent. A peer at another sample rate is converted to Beatlayer's. (S2, S6; conversion and the drop are Beatlayer's own)
- **R7.** Latency runs from 0 to 200 ms, 20 ms on a new install, kept in the preferences and set by `link.set_audio_latency`. Changing it changes how much of each peer's audio is buffered. (S2; the range and the first value are Beatlayer's own)
- **R8.** Sync to Incoming Audio is off on a new install, kept in the preferences and set by `link.set_sync_to_incoming_audio`. On, Beatlayer delays its own playback by the Latency: the moment it gives Link for each block ([Link](link.md) R6) is the Latency later, so its own audio, and the peers' audio it monitors, reach its output in step. Beatlayer doesn't stop two peers having it on. (S2)
- **R9.** The peers list shows each connected peer with Link Audio on, by name, with its state (receiving while a track takes one of its channels and audio arrives, connected otherwise) and the audio buffered from it, in milliseconds. (S2; the states' names are Beatlayer's own)
- **R10.** A track whose input names a peer and channel keeps them, saved with the set by name ([routing](routing.md) R8), while the peer is gone, and is silent; it hears them again when a peer and channel of those names return. (S8; as [MIDI I/O](midi-io.md) R8 keeps a port)
- **R11.** Sending writes each block's samples into the channel's buffer on the audio thread without allocating, locking or blocking. Received buffers arrive on Link's thread, which copies them into a lock-free buffer per channel made when a track takes the input, converting the sample rate there; the audio thread reads them without allocating, locking or blocking. A track that stops taking a channel frees its buffer off the audio thread. (S6, S8)
- **R12.** `link.state` reports Link Audio's switch, Name, Latency, Sync to Incoming Audio and the peers, each with its name, state, buffered audio and channels; `link.set_audio` is `external`, since it sends audio over the network, and `link.set_audio_name`, `link.set_audio_latency` and `link.set_sync_to_incoming_audio` are `edit`, as kept preferences; each names its readback. A track's Link Audio input is set with `routing.set_input` and read back from the track's routing. (S8)
- **R13.** In the accessibility tree the Link Audio controls carry the names they show on the page; the peers list is a table of name, state and buffered audio. (Beatlayer's own)
- **R14.** Two Beatlayer sessions in one test process, on the loopback network, each with Link and Link Audio on, send a track from one and receive it on an audio track of the other, which records it on the beat it was sent on. (Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1, R2, R3 | to be written in WP-7.16 | shell test, UI script |
| R4 | to be written in WP-7.16 | `bl-link` test; engine test |
| R5, R6 | to be written in WP-7.16 | engine test with a stand-in channel; `bl-link` test |
| R7, R8, R9 | to be written in WP-7.16 | engine test, shell test, UI script |
| R10 | to be written in WP-7.16 | shell test |
| R11 | to be written in WP-7.16 | real-time trap, RealtimeSanitizer |
| R12, R13 | to be written in WP-7.16 | agent API, UI script, `cargo xtask commands` |
| R14 | to be written in WP-7.16 | `bl-link` test on the loopback network |

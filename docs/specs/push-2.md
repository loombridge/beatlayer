# Push 2: connecting it, its controls, LEDs and display, and the modes

- **Status:** draft
- **Work packages:** WP-7.19 (R1 to R36). What the pads play in Note Mode, step sequencing, recording on Push 2, and Simpler on Push 2 are [notes on Push 2](push-2-notes.md)'s (WP-7.21 to WP-7.24); browsing, Device mode and racks, mixing, Clip mode and the Session View on Push 2 are [tracks on Push 2](push-2-tracks.md)'s (WP-7.20, WP-7.25 to WP-7.28). The control-surface host, a surface's ports, the Control Surfaces table, instant mappings, the session ring and track banks are [control surfaces](control-surfaces.md)' (WP-7.5, WP-7.6)
- **Parity-matrix row:** Sync and control: Push 2 as a controller
- **Checked against:** the Live 12 reference manual (online edition), read 23 October 2026 (chapter 36 "Using Push 2", all of it; chapter 34, section 34.1.1; chapter 12, section 12.4); Ableton's "Push 2 MIDI and Display Interface Manual", revision 1.1, with its control map, read 23 October 2026; the UI parity reference's row L-SET-014

## The behaviour

Push 2 is a USB controller with 64 pressure-sensitive pads, eleven touch-sensitive encoders, a touch strip, two rows of eight buttons around a 960 × 160 colour display, about fifty more buttons, and two footswitch jacks. Plugged in, it works at once: Beatlayer finds it, takes it over as a control surface, lights its pads and buttons, and draws its display, with nothing to set up in Settings.

Push 2 talks to the computer over two MIDI ports and a display interface. The first MIDI port belongs to the surface, which reads every pad, button, encoder and pedal from it and sends every LED's colour back on it; the second is an ordinary MIDI port the user can map or play from. The display is a USB bulk interface that takes whole frames of pixels, which Beatlayer draws itself, with its own typeface, icons and colours, in the places Live puts things.

What Push 2 does depends on two modes at once. The pad mode, Note or Session, decides what the 64 pads do; the display mode, Device, Browse, Mix or Clip, decides what the display, the encoders and the sixteen display buttons do. Setup, Scale and a few held buttons show menus over the display mode for a while. This spec covers the hardware, the frame Beatlayer draws into, the modes and how they switch, the controls that behave the same in every mode, the Setup menu, footswitches, User Mode, and the simulated Push 2 the tests use. The specs for each mode build on it.

## Sources

| ID | Kind | Source | What it shows |
|---|---|---|---|
| S1 | Manual | Live 12 reference manual, chapter 36 "Using Push 2", the chapter's opening and section 36.1 "Setup" | Push 2 is found as soon as it's connected by USB, needs no driver and no setting, and is usable at once; how it behaves depends on the mode it is in and the kind of track selected |
| S2 | Manual | Section 36.17 "Push 2 Control Reference" | What every button, encoder and the touch strip does: Tap Tempo and the tempo encoder (1 BPM steps, 0.1 BPM with Shift), Metronome and the swing encoder, Undo and Redo with Shift, Play/Stop and its Shift return to the start, the master encoder (the Main track's volume, the cue volume with Shift), Shift for finer encoders, Select, the mode buttons (Device, Mix and Clip momentary when held), Octave buttons unlit at the range's end, and the rest by mode |
| S3 | Manual | Section 36.16 "Setup Menu" | Pad Sensitivity, Pad Gain and Pad Dynamics (0 to 10, default 5) shape the velocity curve, drawn below them with force across and velocity up; Gain 4 with Dynamics 7 is linear; Display Light and LED Brightness (default 100 %); Workflow, Scene (default) or Clip, decides what Duplicate, New and the Up and Down arrows do |
| S4 | Manual | Section 36.17.0.1 "Using Footswitches with Push 2" | Two footswitch jacks: the first is a sustain pedal; a tap of the second toggles Record, a quick double tap acts as New; some footswitches read backwards |
| S5 | Manual | Sections 36.10, 36.11, 36.14 and 36.15 | Device, Mix and Clip enter their modes and are momentary while held in another mode; each Mix press switches Track Mix and Global Mix; Session and Note switch the pad grid and are momentary while held in the other; the lower display buttons select tracks; Shift with a lower display button and a pad of the outer ring colours a track (36.5), and the same picker colours a drum pad (36.3.4.1) and a clip (36.15) |
| S6 | Manual | Sections 36.3.6, 36.9 and 36.12 | A button that switches something on pulses while it is on; the count-in shows as a bar across the top of the display, flashing in time; a small progress bar shows each playing clip's position; a manually armed track has an icon of its own on the display; an automated parameter has a white dot beside it, an overridden one a grey dot |
| S7 | Manual | Chapter 34, section 34.1.1 "Natively Supported Control Surfaces"; chapter 12, section 12.4 | Up to six surfaces in the Control Surfaces table, each chosen by name with an input and an output port; Push 2's pads give polyphonic aftertouch to devices that take MPE |
| S8 | Interface manual | Ableton, "Ableton Push 2 MIDI and Display Interface Manual", revision 1.1 (firmware 1.0.60), sections "MIDI Interface", "MIDI Mode", "MIDI Messages", "MIDI Mapping" (with its control map), "Sysex Commands", "LEDs", "Buttons", "Pads", "Velocity Curve", "Pad Parameters", "Pad Settings", "Aftertouch", "Encoders", "Touch Strip", "Pedals", "Display Backlight", "Device Inquiry", "Statistics", "Display Interface" and Appendix A | The two MIDI ports and their names on each system; Live, User and Dual MIDI modes; every control's note or controller number; the sysex format and commands; LED colour indices, palettes, white balance, brightness and animation by channel, timed by MIDI clock; the velocity curve table; aftertouch modes; relative encoder values and touch notes; the touch strip's configuration flags and LEDs; pedal defaults; display brightness; the identity reply; the display's USB IDs, frame header, pixel format, line layout, XOR pattern, 60 frames a second and blanking after 2 s |
| S9 | Beatlayer | [Control surfaces](control-surfaces.md) (WP-7.5, WP-7.6); [MIDI I/O](midi-io.md); [mixer](mixer.md) R27; [metronome](metronome.md); [automation](automation.md) R5 and R17; [themes](themes.md) R14 to R27; [musical time](musical-time.md); the architecture's `bl-push` row and "Real-time rules"; the UI parity reference's row L-SET-014 | The surface host and a surface's ports; MIDI ports and their switches; the Preview/Cue Volume; the metronome; overriding automation and touch recording; the palette of 70 swatches; the tempo's range; where Push 2's code lives; the Control Surfaces table |

## Rules

**Connecting (WP-7.19)**

- **R1.** A Push 2 is a USB device with vendor ID 0x2982 and product ID 0x1967 whose MIDI ports answer a Device Inquiry with Ableton's manufacturer ID (00 21 1D), device family 0x1967 and family member 2. Beatlayer looks for one while it runs and, when one appears, starts its Push 2 surface on the control-surface host ([control surfaces](control-surfaces.md), WP-7.5) without the user choosing it, whether the device was connected before or after Beatlayer started. Unplugging it stops the surface; plugging it in again starts it afresh and sends every LED and a new frame. (S1, S8, S9)
- **R2.** A Push 2 found this way takes the first empty row of the Control Surfaces table (L-SET-014; [control surfaces](control-surfaces.md) R1, R45), showing the Push 2 surface in the row's chooser and its Live port (R3) as the row's input and output. With every row taken, the surface doesn't start and the Status Bar says why. Setting the row's chooser to None stops the surface, and Beatlayer keeps that choice in the preferences by the port's name, as it keeps the rows ([control surfaces](control-surfaces.md) R2), so it doesn't take a row for that Push 2 again until the user chooses it again (Beatlayer's own). (S7, S9)
- **R3.** Push 2's first MIDI port, the Live port, belongs to the surface ([control surfaces](control-surfaces.md) R3, R4): its messages reach the surface, or a manual mapping that takes them, never a track directly, and only the surface sends to it. Beatlayer recognises it by its name on each system: `Ableton Push 2 Live Port` on macOS, `Ableton Push 2` followed by an optional number on Windows, and the device's first port, `Ableton Push 2 nn:0`, on Linux. The second port, the User port (`Ableton Push 2 User Port`, `MIDIIN2 (Ableton Push 2)` and `MIDIOUT2 (Ableton Push 2)`, or `:1`), is an ordinary port in the MIDI Ports table whose switches are the user's ([MIDI I/O](midi-io.md)). (S8, S9)
- **R4.** Starting the surface ([control surfaces](control-surfaces.md) R9), Beatlayer sends, in order: a Device Inquiry, reading the firmware version; Set MIDI Mode with Live mode; Request Statistics, reading whether Push 2 has its own power supply; the LED palette entries it uses (R14) and Reapply Color Palette; Set LED Brightness and Set Display Brightness from the Setup values (R31); the velocity curve and pad parameters (R30); Set Aftertouch Mode ([notes on Push 2](push-2-notes.md)); the touch strip's configuration (R11); then every LED (R13) and the first frame (R17). It waits for a command's reply before sending the next command that expects one, for at most 500 ms, and carries on without the reply after one retry (Beatlayer's own). Stopping the surface, by quitting, by None in its row or by the user's own choice, turns every LED off, sets the display's brightness to 0 and leaves Push 2 in Live mode. (S8; the order and the timeout are Beatlayer's own)
- **R5.** Beatlayer sends Push 2 only the commands and display messages the interface manual documents, and of those never Flash LED White Balance, Set Temporary 400g Pad Values or Set LED White Balance: it changes nothing that lasts past a restart of the device. It never updates Push 2's firmware; `push.state` reports the firmware version (R34). (S8; Beatlayer's own)

**The controls (WP-7.19)**

- **R6.** The surface reads each control from the Live port as the interface manual's map gives it, and lights its LED with the same number. The table below lists them; "RGB" marks an LED that takes colours, "white" one that takes levels of white. A control's general action is its action in every mode; the specs named in the last column say what it does where it depends on the mode. (S2, S8)

| Control | From Push 2 | LED | In general |
|---|---|---|---|
| 64 pads | Note on 36 to 99 with velocity 1 to 127, note off on release; pressure as R8 says. The pad in row r from the bottom and column c from the left (both from 0) is note 36 + 8r + c | RGB | By pad mode ([notes on Push 2](push-2-notes.md), [tracks on Push 2](push-2-tracks.md)) |
| Eight track encoders | Controllers 71 to 78, left to right; touch on notes 0 to 7 | none | R9; by display mode |
| Master encoder | Controller 79; touch on note 8 | none | R10 |
| Tempo encoder | Controller 14; touch on note 10 | none | R10 |
| Swing encoder | Controller 15; touch on note 9 | none | R10 |
| Touch strip | Touch on note 12; position as pitch bend or controller 1 | 31 white LEDs | R11 |
| Upper display buttons 1 to 8 | Controllers 102 to 109 | RGB | By display mode; labelled in the display's upper band (R18) |
| Lower display buttons 1 to 8 | Controllers 20 to 27 | RGB | Select tracks where a mode doesn't use them otherwise (R26); labelled in the lower band |
| Scene/grid buttons, top to bottom | Controllers 43 down to 36, labelled 1/32t, 1/32, 1/16t, 1/16, 1/8t, 1/8, 1/4t, 1/4 | RGB | Launch scenes in Session Mode; choose the step and repeat rate in Note Mode |
| Tap Tempo | Controller 3 | white | R12 |
| Metronome | Controller 9 | white | R12 |
| Delete | Controller 118 | white | A modifier (R25); what it deletes depends on what is pressed with it |
| Undo | Controller 119 | white | R12 |
| Mute, Solo, Stop Clip | Controllers 60, 61, 29 | RGB | Act on the selected track; modifiers with display buttons and pads; lock on when held (R24) ([tracks on Push 2](push-2-tracks.md)) |
| Convert | Controller 35 | white | Converts what is selected ([notes on Push 2](push-2-notes.md), [tracks on Push 2](push-2-tracks.md)) |
| Double Loop | Controller 117 | white | R12 |
| Quantize | Controller 116 | white | Quantise, or its settings when held ([notes on Push 2](push-2-notes.md)) |
| Duplicate | Controller 88 | white | By Workflow (R32), and a modifier (R25) |
| New | Controller 87 | white | Prepares a new clip, by Workflow ([notes on Push 2](push-2-notes.md)) |
| Fixed Length | Controller 90 | white | Fixed-length recording, its lengths when held ([notes on Push 2](push-2-notes.md)) |
| Automate | Controller 89 | RGB | Automation Arm ([notes on Push 2](push-2-notes.md)) |
| Record | Controller 86 | RGB | Records ([notes on Push 2](push-2-notes.md)); a modifier with lower display buttons |
| Play | Controller 85 | RGB | R12 |
| Setup | Controller 30 | white | R23, R31 |
| User | Controller 59 | white | R33 |
| Add Device, Add Track | Controllers 52, 53 | white | Browse to add a device or a track ([tracks on Push 2](push-2-tracks.md)) |
| Device, Browse, Mix, Clip | Controllers 110, 111, 112, 113 | white | Display modes (R22) |
| Master | Controller 28 | white | Selects the Main track, and the track before it on a second press ([tracks on Push 2](push-2-tracks.md)) |
| Up, Down, Left, Right | Controllers 46, 47, 44, 45 | white | Navigate by mode |
| Repeat, Accent | Controllers 56, 57 | white | Note repeat and full velocity ([notes on Push 2](push-2-notes.md)) |
| Scale | Controller 58 | white | The Scale menu (R23, [notes on Push 2](push-2-notes.md)) |
| Layout | Controller 31 | white | Pad layouts in Note Mode, the Session Overview in Session Mode |
| Note, Session | Controllers 50, 51 | white | Pad modes (R21) |
| Octave Up, Octave Down | Controllers 55, 54 | white | Shift the pads' range by mode; off when the range can't move further |
| Page Left, Page Right | Controllers 62, 63 | white | Pages of steps, or eight tracks in Session Mode |
| Shift, Select | Controllers 49, 48 | white | Modifiers (R25) |
| Footswitch jacks 1 and 2 | Controllers 64 and 69 | none | R28, R29 |

- **R7.** A button sends controller value 127 when pressed and 0 when released. A press released within 300 ms is a tap; one held longer is a hold (Beatlayer's own time). A button that acts on a tap acts when it is released, unless the specs give it an action on the press. (S8; the times are Beatlayer's own)
- **R8.** A pad's note-on carries the velocity Push 2 works out from the force and the velocity curve (R30). While a pad is held, Push 2 sends its pressure as channel pressure (the hardest-pressed pad's) or as polyphonic key pressure on the pad's note, as Set Aftertouch Mode chose; which mode Beatlayer uses, and what the pressure does, is [notes on Push 2](push-2-notes.md)'s. A pad's note-off comes when it's released. (S7, S8)
- **R9.** An encoder sends a relative value in two's complement: 1 to 63 is that many steps clockwise and 127 down to 64 is 1 to 64 steps anticlockwise; Push 2 sends larger values the faster it turns, about 210 steps for a whole turn. Where an encoder controls a parameter, each step moves a continuous parameter by 1/200 of its range, or 1/2000 with Shift held, and stops at the range's ends; a list or stepped parameter moves one value for every 10 steps in one direction (all Beatlayer's own). Touching an encoder sends note-on with velocity 127 and letting go of it velocity 0: from touch to release is one gesture, one undo step however many steps it turned, and the touch that [automation](automation.md) R17 records with. Turning a parameter that has automation, while not recording, overrides it as moving any control does ([automation](automation.md) R5). (S2, S8, S9; the step sizes are Beatlayer's own)
- **R10.** In every mode, the master encoder sets the Main track's volume, and with Shift held the Preview/Cue Volume ([mixer](mixer.md) R27), which the metronome and the browser's preview are heard at. The tempo encoder, which has 18 detents to a turn, moves the song's tempo by 1 BPM a step, or 0.1 BPM with Shift held, within the tempo's range ([musical time](musical-time.md)), as typing in the Control Bar's tempo field does. The swing encoder sets the swing amount [notes on Push 2](push-2-notes.md) uses for Quantize and Repeat, and touching it shows the amount on the display (R19). (S2, S8, S9)
- **R11.** The touch strip sends note 12 on touch and release, and its position as pitch bend (0 at the bottom, 8192 at the centre, only the top 8 of the 14 bits used) or as controller 1 (0 to 127), as its configuration says. Beatlayer gives it one of three configurations as its use needs: pitch bend, with Push 2 lighting one LED at the position and returning to the centre on release (flags 0x68); modulation, with Push 2 lighting a bar from the bottom and staying where it was left (flags 0x0C); and a range, with Beatlayer lighting the 31 LEDs itself with Set Touch Strip LEDs (LED 0 at the bottom, levels 0 to 7) and the strip sending pitch bend (flags 0x03). Which one applies when is [notes on Push 2](push-2-notes.md)'s. (S2, S8; the pairing of uses and flags is Beatlayer's own)
- **R12.** In every mode: Play/Stop starts the transport, or stops it while it runs, as the Control Bar's Play and Stop do; with Shift held while the transport is stopped it moves the song position to 1.1.1 without starting. Metronome switches the metronome on or off. Tap Tempo taps the tempo as the Control Bar's Tap Tempo button does, starting the transport after a bar of taps when Start Transport With Tap Tempo is on (M-PLAY-006c). Undo undoes the last action in the set, whatever made it, and Redo with Shift held. Double Loop runs Duplicate Loop on the selected clip ([multi-clip editing](multi-clip-editing.md) R11). Each runs the same command as the app's own control. (S2, S9)

**LEDs (WP-7.19)**

- **R13.** Beatlayer sets an LED with a note-on (pads) or a controller (buttons) on channel 0 whose value is a colour index, and starts an animation from it with a second message on channels 1 to 15: one-shot fades on 1 to 5, pulsing on 6 to 10 and blinking on 11 to 15, over a 24th, 16th, 8th, quarter or half note. The surface keeps every LED's state and sends only what changed, and sends them all again when it starts or leaves User Mode. A white LED is off when its button does nothing in the present state (Octave buttons at the end of the range, for one), dim (white index 16) when it does something, and full (127) while its function is active or while it is held (the levels are Beatlayer's own). A button that switches something on or off, such as Metronome, Accent or Fixed Length, pulses while it's on. (S2, S6, S8)
- **R14.** Beatlayer writes its own colours into Push 2's palette with Set LED Color Palette Entry, then Reapply Color Palette, rather than relying on the default palette. Index 0 stays black and indices 122 to 127 keep the interface manual's defaults (white, light grey, dark grey, blue, green and red), which the state colours of the other specs use; indices 1 to 121 are given to the other colours as they're needed (track, clip and pad colours from the palette of 70 swatches, [themes](themes.md) R14, and their lighter and dimmer versions), and when all are in use the one shown least recently is rewritten. An entry's white value is its colour's brightness, for the white LEDs. The colours are Beatlayer's own. (S8, S9; the scheme is Beatlayer's own)
- **R15.** Pulsing and blinking follow the song: the surface sends MIDI clock to the Live port, 24 ticks to the quarter note at the song's tempo, each stamped with its moment ([control surfaces](control-surfaces.md) R8), and Start whenever the transport starts, so animations keep in time with the beat; while the transport is stopped the clock carries on at the current tempo. Beatlayer pulses over a quarter note (channel 9) and blinks over an eighth (channel 13) (both Beatlayer's own). (S8, S9)
- **R16.** LED Brightness sets Push 2's global LED brightness to the percentage of 127, and Display Light its display brightness to the percentage of 255 (Set LED Brightness, Set Display Brightness), both rounded. With Push 2 on USB power alone the device limits its own brightness; `push.state` reports the power supply (R34). (S3, S8)

**The display (WP-7.19)**

- **R17.** The display shows 160 lines of 960 pixels. Beatlayer sends a frame as the 16-byte header (FF CC AA 88 and twelve zeros) and then the 160 lines from the top, each 960 pixels from the left in 2 bytes, least significant first, red in bits 0 to 4, green in 5 to 10 and blue in 11 to 15, followed by 128 filler bytes, so each line is 2048 bytes and a frame 327,680; every 4 bytes of a line are XORed with E7, F3, E7 and FF in turn. Frames go as bulk transfers to endpoint 0x01 of interface 0, which Beatlayer claims through `nusb`. They are drawn by `bl-push`'s 2D rasteriser on a thread of the surface's own, never on the audio thread and never through the UI crate; a frame is sent when what it shows has changed, at most 60 times a second, and at least once a second when nothing has, since Push 2 blanks its display 2 s after the last frame. If the display interface can't be claimed, or a transfer fails, the pads, buttons and LEDs carry on, and Beatlayer tries to claim it again every 2 s (Beatlayer's own). (S8, S9)
- **R18.** The display is laid out in eight columns 120 pixels wide, column k under encoder k and between upper display button k and lower display button k, and in three bands: an upper band of 24 pixels holding the upper buttons' labels, a lower band of 24 pixels holding the lower buttons', and the content band between them. A label is drawn in its column with the colour of what it stands for (a track's, a clip's or a device's track's), and the selected one is highlighted in that colour; a display button whose label is empty does nothing and its LED is off, and a button with a label is lit in its colour, dim, and full while selected. Text is in the app's typeface, icons are Beatlayer's own, and the background is black. (S1, S5; the sizes and the look are Beatlayer's own)
- **R19.** Over the bands, the display shows: a count-in bar across the top of the upper band while a count-in runs, flashing on each beat; a progress bar under the label of each lower display button whose track has a clip playing, showing where that clip is in its loop; and notifications, a box in the middle of the content band that shows a short message (the touch strip's new use, the note range, the swing amount, the tempo while its encoder is touched) for 1.5 s (the places and the time are Beatlayer's own). (S2, S6)
- **R20.** Where encoder k controls a parameter, column k's content band shows the parameter's name; its value as text, as the device's own view shows it; a graphic of the value (an arc for a continuous parameter, the chosen entry for a list); a white dot beside the name while the parameter has automation that plays, and a grey dot while that automation is overridden ([automation](automation.md) R5). Touching the encoder highlights the column. A column whose encoder controls nothing is empty. (S6; the look is Beatlayer's own)

**The modes (WP-7.19)**

- **R21.** The pads are in one of two pad modes, Note Mode or Session Mode; Note Mode when the surface starts (Beatlayer's own). Tapping Session or Note switches to that mode. Holding Session in Note Mode, or Note in Session Mode, switches while it's held and back on release. The active pad mode's button is full and the other dim. What the pads do in Note Mode is [notes on Push 2](push-2-notes.md)'s, and in Session Mode [tracks on Push 2](push-2-tracks.md)'. (S2, S5)
- **R22.** The display is in one of four display modes, Device, Browse, Mix or Clip; Device when the surface starts (Beatlayer's own). Tapping Device, Browse, Mix or Clip switches to that mode. Holding Device, Mix or Clip in another mode switches while it's held and back on release; Browse has no hold. Tapping Mix while in Mix switches between Track Mix and Global Mix, Track Mix the first time. The active mode's button is full and the others dim. Both modes are surface state ([control surfaces](control-surfaces.md) R12). A display mode owns the display, the eight track encoders and both rows of display buttons, except where R26 gives the lower buttons to tracks. (S2, S5)
- **R23.** Setup and Scale open menus over the display mode: tapping the button opens it, and tapping it again, or tapping a display mode's button, closes it and leaves the display mode as it was. Fixed Length and Quantize show their menus while held, and Convert shows its choices where there are several ([notes on Push 2](push-2-notes.md)). The button of an open menu is full. Scale is only available in Note Mode with a melodic layout ([notes on Push 2](push-2-notes.md)); elsewhere its LED is off and it does nothing. (S2, S3)
- **R24.** Mute, Solo and Stop Clip held alone for 1 s lock on (Beatlayer's own time) until pressed again; what they do while held or locked is [tracks on Push 2](push-2-tracks.md)'s and, with pads and steps, [notes on Push 2](push-2-notes.md)'s. (S2)
- **R25.** Shift, Select, Delete, Duplicate, Mute, Solo, Stop Clip, Quantize, Record and Layout also act as modifiers. A control pressed while a modifier is held does the combination the specs give it, if there is one, and not its own action; and a modifier used in a combination doesn't do its own action when released. With Shift held, a button does its Shift action, an encoder moves finer, and the other modifiers do theirs. (S2; the precedence is Beatlayer's own)
- **R26.** In Note Mode, in Session Mode, and in the display modes that leave them free, the lower display buttons stand for the eight tracks of the surface's session ring, eight tracks wide and eight scenes tall ([control surfaces](control-surfaces.md) R28, R33), each labelled with the track's name in its colour, and the selected track's highlighted. Pressing one selects that track as clicking its title bar does (`session.select`, [control surfaces](control-surfaces.md) R34), and the device the surface controls follows ([control surfaces](control-surfaces.md) R17). Shift and a lower display button opens the colour picker for that track (R27). (S2, S5)
- **R27.** The colour picker, for a track (R26), a drum pad and a clip (the specs that open it), lights the 28 pads of the grid's outer ring with the swatches of the palette's first two rows, 14 each, clockwise from the bottom-left pad (Beatlayer's own choice and order), and turns the 36 inner pads off; the target's present colour blinks. Tapping a ring pad gives the target that colour as one undo step, and closes the picker; any button closes it without a change. (S5, S9; the swatches and their order are Beatlayer's own)

**Footswitches (WP-7.19)**

- **R28.** Footswitch 1 is a sustain pedal: Beatlayer passes its down and up as controller 64 values 127 and 0 into the selected track's input, as a keyboard's sustain pedal reaches a track ([MIDI I/O](midi-io.md)), so it sustains the notes played on the pads and records with them. (S4, S8)
- **R29.** A tap of footswitch 2 acts as a press of Record. Two taps within 300 ms act as a press of New instead, so Beatlayer waits 300 ms after a first tap before acting on it (both Beatlayer's own). A footswitch reads down at controller value 127 and up at 0; Beatlayer doesn't invert a footswitch that reads backwards. (S4, S8)

**Setup and Workflow (WP-7.19)**

- **R30.** From Pad Sensitivity s, Pad Gain g and Pad Dynamics d (each 0 to 10), Beatlayer works out the 128-entry velocity curve, entry i being the velocity for a force of 32·i grams, and sends it with eight Set Pad Velocity Curve Entry commands, again whenever a value changes. With x = i/127, x₁ = min(1, x·2^((s−5)/2.5)), x₂ = x₁^(2^((4−g)/3)), and x₃ = 0.5 + sign(x₂−0.5)·|2x₂−1|^(2^((7−d)/3))/2, the entry is round(1 + 126·x₃) (Beatlayer's own curve): higher sensitivity reaches a high velocity with less force, Gain raises the middle of the curve most, higher Dynamics pushes velocities toward both ends and lower Dynamics toward the middle, and Gain 4 with Dynamics 7 and Sensitivity 5 is a straight line. Beatlayer also sends Set Pad Parameters with the aftertouch thresholds 1220 and 1570 (Beatlayer's own values). (S3, S8)
- **R31.** Setup opens the Setup menu: Pad Sensitivity, Pad Gain and Pad Dynamics on encoders 1 to 3, the velocity curve drawn over columns 4 and 5 with force across and velocity up, Display Light and LED Brightness (0 to 100 %, default 100) on encoders 6 and 7, and Workflow (Scene or Clip, default Scene) on encoder 8 (the places are Beatlayer's own). The values are preferences, kept between launches and the same for every set; `push.set_setup` sets them. (S3)
- **R32.** Workflow decides what Duplicate, New and the Up and Down arrows do in Note Mode ([notes on Push 2](push-2-notes.md)) and nothing else. (S3)

**User Mode (WP-7.19)**

- **R33.** Tapping User puts Push 2 in User mode (Set MIDI Mode, User): the device sends its controls on the User port, where they reach tracks or MIDI mapping as that port's switches allow, and the surface acts on nothing from the Live port but User. Beatlayer turns every LED off except User, which is full, and draws a screen saying Push 2 is in User Mode. Tapping User again sets Live mode, and the surface sends every LED and a frame and carries on in the modes it had. The User button reaches the surface in both modes, since Push 2 sends it on both ports. (S2, S8; the screen is Beatlayer's own)

**Agents and tests (WP-7.19)**

- **R34.** `push.state` (`read`) reports each connected Push 2: its ports, firmware version and power supply; its MIDI mode; the pad mode, the display mode (with Track Mix or Global Mix), any open menu and the buttons held or locked; each LED's colour or white level and animation, by control name; the touch strip's configuration and LEDs; the Setup values; and the display's content as text, by band and column (labels, parameter names and values, dots, notifications). `push.set_mode` (`read`) sets the pad mode and the display mode as their buttons do; it changes nothing in the set. `push.set_setup` (`edit`, a preference) sets R31's values, read back by `push.state`. `push.display_frame` (`read`) returns the latest frame as a PNG. Every action a Push 2 control takes in the set runs a registered command, the same one the app's own control runs, with the same undo step. (S9)
- **R35.** Nothing for Push 2 runs on the audio thread: the surface runs on the host's thread, the display on its own (R17), and the notes the pads play reach tracks on the same lock-free input queue as a MIDI port's, stamped with the time the Live port received them ([MIDI I/O](midi-io.md)). (S9)
- **R36.** A simulated Push 2 stands in for the device in tests: the test MIDI backend's virtual Live and User ports with the macOS names; a responder that answers Device Inquiry, Request Statistics and each Get command as the interface manual's examples do, and Set MIDI Mode with its reply on both ports; a model of every LED that applies each note-on, controller, animation and touch strip message it receives; and a frame sink in place of the USB display that undoes the XOR and the pixel format and keeps each frame as RGB. A test presses and releases pads with velocity and pressure, turns and touches encoders, moves the touch strip, presses buttons and footswitches, and reads LEDs, sysex and frames, with no device and no window. (S8; Beatlayer's own)

## Tests

| Rule | Test | Kind |
|---|---|---|
| R1 | to be written in WP-7.19 | unit (simulated Push 2) |
| R2 | to be written in WP-7.19 | unit, UI script |
| R3 | to be written in WP-7.19 | unit |
| R4 | to be written in WP-7.19 | unit (simulated Push 2) |
| R5 | to be written in WP-7.19 | unit |
| R6 | to be written in WP-7.19 | unit (simulated Push 2) |
| R7 | to be written in WP-7.19 | unit |
| R8 | to be written in WP-7.19 | unit |
| R9 | to be written in WP-7.19 | unit, agent |
| R10 | to be written in WP-7.19 | unit, agent |
| R11 | to be written in WP-7.19 | unit (simulated Push 2) |
| R12 | to be written in WP-7.19 | unit, agent |
| R13 | to be written in WP-7.19 | unit (simulated Push 2) |
| R14 | to be written in WP-7.19 | unit |
| R15 | to be written in WP-7.19 | unit |
| R16 | to be written in WP-7.19 | unit |
| R17 | to be written in WP-7.19 | unit, golden frame |
| R18 | to be written in WP-7.19 | golden frame |
| R19 | to be written in WP-7.19 | golden frame |
| R20 | to be written in WP-7.19 | golden frame |
| R21 | to be written in WP-7.19 | unit, agent |
| R22 | to be written in WP-7.19 | unit, agent |
| R23 | to be written in WP-7.19 | unit |
| R24 | to be written in WP-7.19 | unit |
| R25 | to be written in WP-7.19 | unit |
| R26 | to be written in WP-7.19 | unit, agent |
| R27 | to be written in WP-7.19 | unit |
| R28 | to be written in WP-7.19 | unit |
| R29 | to be written in WP-7.19 | unit, property |
| R30 | to be written in WP-7.19 | unit, golden frame, agent |
| R31 | to be written in WP-7.19 | unit |
| R32 | to be written in WP-7.19 | unit (simulated Push 2) |
| R33 | to be written in WP-7.19 | agent |
| R34 | to be written in WP-7.19 | unit |
| R35 | to be written in WP-7.19 | real-time trap |
| R36 | to be written in WP-7.19 | unit |
